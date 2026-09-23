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
//! other to the person arranging the level. A piece with compound collision — a cave, an
//! overhang — is picked against the box around all of its parts.
//!
//! # Things with no geometry at all
//!
//! Water, spawn points and lights are picked against a box that exists only here: water's is
//! its volume, a spawn point's is a person-sized box standing on it, and a light's is a small
//! cube around where it shines from. None of them has a mesh — what an author clicks on is
//! the marker drawn for it, and the box is that marker's size.
//!
//! # Water
//!
//! Water blocks nothing and never will — see [`engine_environment::water`]. It is picked
//! against its *volume*: the rectangle, turned by its yaw, reaching down by its depth. The
//! same oriented-box test, given a box that exists only here.
//!
//! # Surfaces
//!
//! [`surface_under`] answers a different question from the pick: not "what did the author
//! click" but "what would something dropped here stand on". Only the *top* of a solid box
//! counts, so an asset dropped onto a crate lands on the crate and one dropped at a wall's
//! face lands on the ground in front of it rather than half way up the wall.

use engine_core::math::{Quat, Vec3};
use engine_environment::lighting::SceneLight;
use engine_environment::object::EnvironmentCollision;
use engine_environment::spawn::SpawnPoint;
use engine_environment::water::WaterBody;
use engine_environment::{EnvironmentObject, EnvironmentScene};

use crate::Selected;

/// Half-extents given to an object with no collision box, in metres.
///
/// Roughly a fern: large enough to hit at a normal working distance, small enough not to
/// swallow the clicks meant for what is behind it.
const DECORATION_HALF_EXTENTS: Vec3 = Vec3::new(0.6, 0.9, 0.6);

/// Half the size of the cube a light is picked by, in metres.
pub const LIGHT_HALF_EXTENT: f32 = 0.35;

/// The pickable box of an object, in world space: centre, half-extents and rotation.
fn pick_box(scene: &EnvironmentScene, object: &EnvironmentObject) -> (Vec3, Vec3, Quat) {
    if let Some(world) = object.world_collision() {
        return (world.center, world.half_extents, world.rotation);
    }
    // A compound piece is picked by the box around its parts, in its own space, so a turned
    // cave piece is outlined and hit the way it is turned.
    if let EnvironmentCollision::Compound(index) = object.collision
        && let Some(shape) = scene.collision_shapes.get(index as usize)
        && !shape.boxes.is_empty()
    {
        let mut low = Vec3::splat(f32::INFINITY);
        let mut high = Vec3::splat(f32::NEG_INFINITY);
        for part in &shape.boxes {
            low = low.min(part.center - part.half_extents);
            high = high.max(part.center + part.half_extents);
        }
        let world = object.place_box(engine_environment::object::CollisionBox::new(
            (low + high) * 0.5,
            (high - low) * 0.5,
        ));
        return (world.center, world.half_extents, world.rotation);
    }
    let scale = object.transform.scale;
    (
        object.position() + Vec3::new(0.0, DECORATION_HALF_EXTENTS.y * scale.y, 0.0),
        Vec3::new(
            DECORATION_HALF_EXTENTS.x * scale.x,
            DECORATION_HALF_EXTENTS.y * scale.y,
            DECORATION_HALF_EXTENTS.z * scale.z,
        ),
        object.transform.rotation,
    )
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
    rotation: Quat,
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

/// The pickable box of a body of water: its volume, as centre, half-extents and rotation.
///
/// The volume rather than the surface, so a lake can be clicked on from under it as well as
/// from above — an author looking up at a pond from a cave mouth is still pointing at it.
fn water_box(body: &WaterBody) -> (Vec3, Vec3, Quat) {
    let half_height = body.depth * 0.5;
    (
        Vec3::new(body.center.x, body.center.y - half_height, body.center.z),
        Vec3::new(body.half_extents.x, half_height, body.half_extents.y),
        Quat::from_yaw_pitch_roll(body.yaw, 0.0, 0.0),
    )
}

/// The pickable box of a spawn point: roughly the person who will stand on it.
///
/// A metre across and two tall, which is the character controller's own capsule rounded to
/// something an author can hit at a working distance. It is turned by the spawn's yaw so that
/// clicking a marker that has been turned still hits it.
fn spawn_box(spawn: &SpawnPoint) -> (Vec3, Vec3, Quat) {
    const HALF: Vec3 = Vec3::new(0.5, 1.0, 0.5);
    (
        spawn.position + Vec3::new(0.0, HALF.y, 0.0),
        HALF,
        Quat::from_yaw_pitch_roll(spawn.yaw, 0.0, 0.0),
    )
}

/// The pickable box of a light: a small cube around where it shines from.
fn light_box(light: &SceneLight) -> (Vec3, Vec3, Quat) {
    (
        light.position,
        Vec3::splat(LIGHT_HALF_EXTENT),
        Quat::IDENTITY,
    )
}

/// The box that stands for something selected, in world space: centre, half-extents, rotation.
///
/// The same box the ray is tested against, which is the point: what an author clicks on and
/// what the editor draws a highlight around have to be the same shape, or the highlight is a
/// second opinion about where the object is.
#[must_use]
pub fn selection_box(scene: &EnvironmentScene, what: Selected) -> Option<(Vec3, Vec3, Quat)> {
    match what {
        Selected::Object(index) => scene
            .objects
            .get(index)
            .map(|object| pick_box(scene, object)),
        Selected::Water(index) => scene.water.get(index).map(water_box),
        Selected::Spawn(index) => scene.spawn_points.get(index).map(spawn_box),
        Selected::Light(index) => scene.lights.get(index).map(light_box),
    }
}

/// Everything the level holds that an author can point at, in hierarchy order.
fn everything(scene: &EnvironmentScene) -> impl Iterator<Item = Selected> + '_ {
    (0..scene.objects.len())
        .map(Selected::Object)
        .chain((0..scene.water.len()).map(Selected::Water))
        .chain((0..scene.spawn_points.len()).map(Selected::Spawn))
        .chain((0..scene.lights.len()).map(Selected::Light))
}

/// What a ray hits first: a placement, a body of water, a spawn point, a light, or nothing.
///
/// One function over every list rather than one per list that a caller has to compare,
/// because "which is nearer, the rock or the pond" is exactly the question a click asks and
/// exactly the one separate answers cannot settle. Ties go to the placement, which is the
/// earliest list.
#[must_use]
pub fn under_ray(scene: &EnvironmentScene, origin: Vec3, direction: Vec3) -> Option<Selected> {
    under_ray_where(scene, origin, direction, |_| true)
}

/// [`under_ray`], over only the things `pickable` accepts.
///
/// What a hidden or locked object is left out by: it is still in the level, and a click
/// passes through it to whatever is behind.
#[must_use]
pub fn under_ray_where(
    scene: &EnvironmentScene,
    origin: Vec3,
    direction: Vec3,
    pickable: impl Fn(Selected) -> bool,
) -> Option<Selected> {
    let mut best: Option<(Selected, f32)> = None;
    for what in everything(scene) {
        if !pickable(what) {
            continue;
        }
        let Some((center, half_extents, rotation)) = selection_box(scene, what) else {
            continue;
        };
        if let Some(distance) = ray_box_distance(origin, direction, center, half_extents, rotation)
            && best.is_none_or(|(_, nearest)| distance < nearest)
        {
            best = Some((what, distance));
        }
    }
    best.map(|(what, _)| what)
}

/// Everything whose pickable box has its centre inside `contains`, which is a test on the
/// screen position of a world point.
///
/// What a box selection asks: an author dragging a rectangle around a row of rocks means the
/// rocks whose middles are inside it, not every rock whose box it grazes.
#[must_use]
pub fn inside_screen_rect(
    scene: &EnvironmentScene,
    contains: impl Fn(Vec3) -> bool,
    pickable: impl Fn(Selected) -> bool,
) -> Vec<Selected> {
    everything(scene)
        .filter(|what| pickable(*what))
        .filter(|what| selection_box(scene, *what).is_some_and(|(center, _, _)| contains(center)))
        .collect()
}

/// Which object a ray hits first, if any.
///
/// Nearest wins, and ties go to the earlier placement — which is stable, because a scene's
/// object order is stable.
#[must_use]
pub fn object_under_ray(scene: &EnvironmentScene, origin: Vec3, direction: Vec3) -> Option<usize> {
    under_ray_where(scene, origin, direction, |what| {
        matches!(what, Selected::Object(_))
    })
    .and_then(Selected::object)
}

/// The top of a solid thing a ray lands on, if one is nearer than `ground`.
///
/// Every box a placement blocks with is a candidate — each part of a compound piece on its
/// own, so a crate dropped into a cave room lands on the floor slab and not on the roof — but
/// only where the ray enters through the box's upward face. `ignore` leaves out what is being
/// moved, so a selection dropped onto the surface under it does not land on itself.
#[must_use]
pub fn surface_under(
    scene: &EnvironmentScene,
    origin: Vec3,
    direction: Vec3,
    ground: Option<Vec3>,
    ignore: impl Fn(usize) -> bool,
) -> Option<Vec3> {
    let length = direction.length();
    if length <= 1e-9 {
        return ground;
    }
    let mut best = ground.map(|point| ((point - origin).length() / length, point));
    for (index, object) in scene.objects.iter().enumerate() {
        if ignore(index) {
            continue;
        }
        for part in scene.world_boxes(object) {
            let Some(t) = ray_box_distance(
                origin,
                direction,
                part.center,
                part.half_extents,
                part.rotation,
            ) else {
                continue;
            };
            let point = origin + direction * t;
            // The top face in the box's own space: yaw-only rotation keeps it level, but the
            // test is written in the box's frame so a pitched box would still answer honestly.
            let local = part.rotation.conjugate().rotate(point - part.center);
            if local.y < part.half_extents.y - 0.02 {
                continue;
            }
            if best.is_none_or(|(nearest, _)| t < nearest) {
                best = Some((t, point));
            }
        }
    }
    best.map(|(_, point)| point)
}

/// Where something standing at `position` would come to rest if it fell straight down.
///
/// The highest top face under it that is below `position.y + 0.01`, or `ground` if there is
/// none. `ignore` leaves out what is falling.
#[must_use]
pub fn rest_below(
    scene: &EnvironmentScene,
    position: Vec3,
    ground: f32,
    ignore: impl Fn(usize) -> bool,
) -> f32 {
    let mut best = ground;
    for (index, object) in scene.objects.iter().enumerate() {
        if ignore(index) {
            continue;
        }
        for part in scene.world_boxes(object) {
            let local = part.rotation.conjugate().rotate(position - part.center);
            if local.x.abs() > part.half_extents.x || local.z.abs() > part.half_extents.z {
                continue;
            }
            let top = part.center.y + part.half_extents.y;
            if top <= position.y + 0.01 && top > best {
                best = top;
            }
        }
    }
    best
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
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
        scene.push(EnvironmentObject::decoration(
            FERN,
            Vec3::new(6.0, 0.0, -10.0),
        ));
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
        assert_eq!(
            hit,
            Some(2),
            "a fern the character walks through is still an object"
        );
    }

    #[test]
    fn a_click_answers_with_whichever_of_the_two_lists_is_nearer() {
        use engine_core::math::Vec2;

        let mut scene = scene();
        // A pond between the eye and the near pillar, and one behind the far one.
        scene.push_water(WaterBody::new(
            Vec3::new(0.0, 1.0, -5.0),
            Vec2::new(3.0, 3.0),
            2.0,
        ));
        scene.push_water(WaterBody::new(
            Vec3::new(0.0, 1.0, -50.0),
            Vec2::new(3.0, 3.0),
            2.0,
        ));

        let hit = under_ray(&scene, Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(
            hit,
            Some(Selected::Water(0)),
            "the pond is nearer than the pillar behind it"
        );

        // Above the pond's surface and pointed at the pillar: the pond's volume is not in the
        // way, so the placement wins. A pick that compared only footprints would say water.
        let hit = under_ray(&scene, Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(hit, Some(Selected::Object(0)));
    }

    #[test]
    fn a_turned_body_of_water_is_picked_in_its_own_space() {
        use engine_core::math::Vec2;

        let mut scene = EnvironmentScene::empty("water_pick");
        scene.push_water(
            WaterBody::new(Vec3::new(0.0, 1.0, 0.0), Vec2::new(12.0, 2.0), 2.0)
                .with_yaw(std::f32::consts::FRAC_PI_2),
        );
        // Ten metres out along Z, which the turned body spans and the unturned one does not.
        let hit = under_ray(
            &scene,
            Vec3::new(20.0, 0.5, 10.0),
            Vec3::new(-1.0, 0.0, 0.0),
        );
        assert_eq!(hit, Some(Selected::Water(0)));

        let miss = under_ray(&scene, Vec3::new(20.0, 0.5, 0.0), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(miss, None, "ten metres out along X is past its end now");
    }

    #[test]
    fn a_spawn_point_can_be_clicked_on_even_though_it_has_no_mesh() {
        let mut scene = EnvironmentScene::empty("spawn_pick");
        scene.push_spawn(SpawnPoint::new(Vec3::new(0.0, 0.0, -10.0)));
        let hit = under_ray(&scene, Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(hit, Some(Selected::Spawn(0)));

        // And it is only as big as the marker: a click well to the side of it hits nothing,
        // rather than a spawn point swallowing every click in its half of the level.
        let miss = under_ray(&scene, Vec3::new(4.0, 1.0, 0.0), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(miss, None);
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
        let hit = object_under_ray(&scene(), Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
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
        let (center, half_extents, rotation) = pick_box(&EnvironmentScene::empty("long"), &long);
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
        assert!(
            past_the_end.is_none(),
            "the wall does not reach twelve metres"
        );
    }

    #[test]
    fn a_light_is_picked_by_the_cube_around_it_and_nothing_further_out() {
        let mut scene = EnvironmentScene::empty("light_pick");
        scene.push_light(SceneLight::brazier(Vec3::new(0.0, 2.0, -10.0), 8.0));
        let hit = under_ray(&scene, Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(hit, Some(Selected::Light(0)));
        let miss = under_ray(&scene, Vec3::new(1.0, 2.0, 0.0), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(miss, None, "a light is a marker, not the room it lights");
    }

    #[test]
    fn a_click_passes_through_what_the_filter_leaves_out() {
        let hit = under_ray_where(
            &scene(),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            |what| what != Selected::Object(0),
        );
        assert_eq!(
            hit,
            Some(Selected::Object(1)),
            "the far pillar, behind the left-out one"
        );
    }

    #[test]
    fn a_drop_lands_on_the_top_of_a_box_and_never_on_its_side() {
        let scene = scene();
        // Straight down onto the near pillar: its top, well above the ground.
        let top = surface_under(
            &scene,
            Vec3::new(0.0, 20.0, -10.0),
            Vec3::new(0.0, -1.0, 0.0),
            Some(Vec3::new(0.0, 0.0, -10.0)),
            |_| false,
        )
        .expect("a surface");
        assert!(top.y > 1.0, "landed at {top:?}, not on the pillar");

        // Level, into the pillar's side: that is not something to stand on, so the ground wins.
        let side = surface_under(
            &scene,
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, -0.05, -1.0),
            Some(Vec3::new(0.0, 0.0, -20.0)),
            |_| false,
        )
        .expect("a surface");
        assert!(
            side.y.abs() < 1e-3,
            "landed at {side:?}, half way up a wall"
        );

        // And what is being moved is never its own floor.
        let ignored = surface_under(
            &scene,
            Vec3::new(0.0, 20.0, -10.0),
            Vec3::new(0.0, -1.0, 0.0),
            Some(Vec3::new(0.0, 0.0, -10.0)),
            |index| index == 0,
        )
        .expect("a surface");
        assert!(ignored.y.abs() < 1e-3);
    }

    #[test]
    fn something_dropped_comes_to_rest_on_the_highest_top_below_it() {
        let scene = scene();
        let rest = rest_below(&scene, Vec3::new(0.0, 30.0, -10.0), 0.0, |_| false);
        assert!(
            rest > 1.0,
            "it should stand on the pillar, rested at {rest}"
        );
        let beside = rest_below(&scene, Vec3::new(3.0, 30.0, -10.0), 0.0, |_| false);
        assert_eq!(beside, 0.0);
    }
}
