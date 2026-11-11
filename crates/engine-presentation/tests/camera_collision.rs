//! The third-person camera never ends up inside, behind or under solid geometry.
//!
//! ```text
//!   character (P)----------o eye          nothing in the way: the eye sits at the zoom
//!   character (P)-----o  | wall  .        a wall: the eye stops in front of it
//! ```
//!
//! Every case is the real camera against a real [`CollisionWorld`], asked the way the game
//! asks it: `follow`, then `avoid_obstructions`. What "valid" means is the same everywhere
//! and is written once, in [`assert_valid`]: the camera's sphere touches nothing, the eye can
//! see the pivot, and the pivot can see the character.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_collision::heightfield::Heightfield;
use engine_collision::shape::{Capsule, Obb, Segment, Shape};
use engine_collision::world::{QueryBuffers, StaticDesc};
use engine_collision::{CollisionMask, CollisionWorld, Ray};
use engine_core::math::{Quat, Vec3};
use engine_presentation::camera::ThirdPersonCamera;

/// One frame at 60 FPS.
const FRAME: f32 = 1.0 / 60.0;

/// A world with a floor at zero and whatever boxes are given, ready to query.
fn world_with(boxes: &[(Vec3, Vec3)]) -> CollisionWorld {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    for &(min, max) in boxes {
        world.add_box(min, max, "BOX");
    }
    world.rebuild_broad_phase();
    world
}

/// A camera following a character at `character`, looking from `yaw`/`pitch` at `distance`,
/// already placed against `world`.
fn camera_at(
    world: &CollisionWorld,
    character: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
) -> ThirdPersonCamera {
    let mut camera = ThirdPersonCamera::following(character);
    camera.yaw = yaw;
    camera.pitch = pitch;
    camera.distance = distance;
    camera.avoid_obstructions(world, character, 0.0);
    camera
}

/// The one definition of a camera in a legal place.
///
/// - The camera's own sphere touches nothing solid, unless it was boxed in below its
///   minimum distance — in which case its centre must still be in front of the surface.
/// - Nothing solid stands between the eye and the pivot it orbits, or between the pivot and
///   the character it follows: the eye is on the character's side of every wall.
/// - The view still points from the eye at the pivot: a wall shortens the orbit and never
///   turns it.
fn assert_valid(world: &CollisionWorld, camera: &ThirdPersonCamera, character: Vec3, case: &str) {
    let eye = camera.eye();
    let pivot = camera.pivot();
    let anchor = character + Vec3::new(0.0, 1.4, 0.0);
    let mask = CollisionMask::WORLD;
    assert!(
        eye.x.is_finite() && eye.y.is_finite() && eye.z.is_finite(),
        "{case}: the eye is {eye:?}"
    );

    let mut buffers = QueryBuffers::new();
    let sphere = Capsule::new(Segment::new(eye, eye), camera.collision.radius);
    if camera.actual_distance() > camera.collision.min_distance + 1e-3 {
        assert!(
            !world.capsule_overlaps(&sphere, mask, &mut buffers),
            "{case}: the camera's sphere at {eye:?} is inside geometry: {:?}",
            buffers.contacts()
        );
    }
    if let Some(hit) = world.segment_blocked(pivot, eye, mask) {
        panic!(
            "{case}: a wall stands between the pivot and the eye at {:?}",
            hit.hit.point
        );
    }
    if let Some(hit) = world.segment_blocked(anchor, pivot, mask) {
        panic!(
            "{case}: a wall stands between the character and the pivot at {:?}",
            hit.hit.point
        );
    }
    // Not below the floor, ever.
    assert!(eye.y > 0.0, "{case}: the eye is under the floor at {eye:?}");

    if camera.actual_distance() > 1e-3 {
        let view = (pivot - eye).normalize();
        let forward = camera.rotation().rotate(-Vec3::Z);
        assert!(
            view.dot(forward) > 0.999,
            "{case}: the eye no longer looks at the pivot"
        );
    }
}

#[test]
fn with_nothing_in_the_way_the_camera_sits_at_its_zoom() {
    let world = world_with(&[]);
    let camera = camera_at(&world, Vec3::ZERO, 0.0, -0.22, 8.0);
    assert!((camera.actual_distance() - 8.0).abs() < 1e-6);
    assert!(
        (camera.eye() - (camera.focus() + camera.rotation().rotate(Vec3::Z) * 8.0)).length() < 1e-5
    );
    let probe = camera.probe().expect("the check ran");
    assert!(!probe.obstructed());
    assert!((probe.limit - 8.0).abs() < 1e-6);
    assert_valid(&world, &camera, Vec3::ZERO, "open ground");
}

#[test]
fn a_wall_behind_the_character_pulls_the_camera_in_front_of_it() {
    // The camera looks down -Z from +Z; the wall's near face is 3 m behind.
    let world = world_with(&[(Vec3::new(-10.0, 0.0, 3.0), Vec3::new(10.0, 6.0, 4.0))]);
    let camera = camera_at(&world, Vec3::ZERO, 0.0, -0.22, 8.0);
    let eye = camera.eye();
    let config = camera.collision;
    assert!(
        eye.z <= 3.0 - config.radius - config.padding * 0.9,
        "the eye at {eye:?} is not in front of the wall"
    );
    // Right up to it, not timidly short of it.
    assert!(
        eye.z > 3.0 - config.radius - config.padding - 0.05,
        "{eye:?}"
    );
    assert!(camera.actual_distance() < 8.0);
    assert_eq!(camera.distance, 8.0, "the player's zoom is not touched");
    let probe = camera.probe().expect("the check ran");
    let hit = probe.hit.expect("the wall was hit");
    assert!((hit.point.z - 3.0).abs() < 0.05, "{:?}", hit.point);
    assert!(
        hit.normal.z < -0.99,
        "the normal faces back at the character"
    );
    assert_valid(&world, &camera, Vec3::ZERO, "wall behind");
}

#[test]
fn looking_up_never_puts_the_camera_under_the_floor() {
    for pitch in [0.0, 0.3, 0.6, 0.9, 1.1] {
        let world = world_with(&[]);
        let camera = camera_at(&world, Vec3::ZERO, 0.0, pitch, 12.0);
        assert!(
            camera.eye().y >= camera.collision.radius - 1e-3,
            "pitch {pitch}: the eye is at {:?}",
            camera.eye()
        );
        assert_valid(
            &world,
            &camera,
            Vec3::ZERO,
            &format!("floor, pitch {pitch}"),
        );
    }
}

#[test]
fn looking_down_under_a_low_ceiling_keeps_the_camera_below_it() {
    // 2.6 m of headroom: the character fits, an orbit at 8 m looking down does not.
    let world = world_with(&[(Vec3::new(-20.0, 2.6, -20.0), Vec3::new(20.0, 4.0, 20.0))]);
    for pitch in [-1.2, -0.8, -0.4] {
        let camera = camera_at(&world, Vec3::ZERO, 0.0, pitch, 8.0);
        assert!(
            camera.eye().y <= 2.6 - camera.collision.radius + 1e-3,
            "pitch {pitch}: the eye at {:?} is in the ceiling",
            camera.eye()
        );
        assert_valid(
            &world,
            &camera,
            Vec3::ZERO,
            &format!("ceiling, pitch {pitch}"),
        );
    }
}

#[test]
fn in_a_corner_the_camera_stays_clear_of_both_walls() {
    // A wall behind (+Z) and a wall to the right (+X), meeting at a corner; the camera
    // looks from the corner's direction.
    let world = world_with(&[
        (Vec3::new(-10.0, 0.0, 2.0), Vec3::new(10.0, 6.0, 3.0)),
        (Vec3::new(1.5, 0.0, -10.0), Vec3::new(2.5, 6.0, 10.0)),
    ]);
    for step in 0..16 {
        // Yaw sweeps the quarter that faces into the corner, and past it on both sides.
        let yaw = -0.4 - step as f32 * 0.1;
        let camera = camera_at(&world, Vec3::ZERO, yaw, -0.3, 10.0);
        let eye = camera.eye();
        assert!(
            eye.x <= 1.5 - camera.collision.radius + 1e-3,
            "yaw {yaw}: {eye:?}"
        );
        assert!(
            eye.z <= 2.0 - camera.collision.radius + 1e-3,
            "yaw {yaw}: {eye:?}"
        );
        assert_valid(&world, &camera, Vec3::ZERO, &format!("corner, yaw {yaw}"));
    }
}

#[test]
fn in_a_narrow_corridor_the_camera_stays_between_the_walls() {
    // Two metres wide, three high, running along Z.
    let world = world_with(&[
        (Vec3::new(-2.0, 0.0, -30.0), Vec3::new(-1.0, 3.0, 30.0)),
        (Vec3::new(1.0, 0.0, -30.0), Vec3::new(2.0, 3.0, 30.0)),
        (Vec3::new(-2.0, 3.0, -30.0), Vec3::new(2.0, 4.0, 30.0)),
    ]);
    // Straight down the corridor at a shallow angle, the way is clear for a while.
    let along = camera_at(&world, Vec3::ZERO, 0.0, -0.05, 8.0);
    assert_valid(&world, &along, Vec3::ZERO, "down the corridor");
    for step in 0..36 {
        let yaw = step as f32 * std::f32::consts::TAU / 36.0;
        for pitch in [-1.0, -0.3, 0.2, 0.8] {
            let camera = camera_at(&world, Vec3::ZERO, yaw, pitch, 15.0);
            let eye = camera.eye();
            assert!(
                eye.x.abs() <= 1.0 - camera.collision.radius + 1e-3,
                "{yaw}/{pitch}: {eye:?}"
            );
            assert!(
                eye.y <= 3.0 - camera.collision.radius + 1e-3,
                "{yaw}/{pitch}: {eye:?}"
            );
            assert_valid(
                &world,
                &camera,
                Vec3::ZERO,
                &format!("corridor {yaw}/{pitch}"),
            );
        }
    }
}

#[test]
fn when_the_wall_goes_the_camera_eases_back_out_rather_than_snapping() {
    let walled = world_with(&[(Vec3::new(-10.0, 0.0, 3.0), Vec3::new(10.0, 6.0, 4.0))]);
    let open = world_with(&[]);
    let mut camera = camera_at(&walled, Vec3::ZERO, 0.0, -0.22, 8.0);
    let held = camera.actual_distance();
    assert!(held < 3.5, "{held}");

    // One frame after the wall is gone, the camera has started back but is nowhere near.
    camera.follow(Vec3::ZERO, FRAME);
    camera.avoid_obstructions(&open, Vec3::ZERO, FRAME);
    let first = camera.actual_distance();
    assert!(first > held, "it did not start back");
    assert!(first < held + 1.0, "it snapped back: {held} -> {first}");

    // Then it keeps coming, never past the zoom, and arrives.
    let mut previous = first;
    let mut frames = 0;
    while camera.actual_distance() < 8.0 && frames < 600 {
        camera.follow(Vec3::ZERO, FRAME);
        camera.avoid_obstructions(&open, Vec3::ZERO, FRAME);
        let now = camera.actual_distance();
        assert!(now >= previous, "it moved back in with nothing in the way");
        assert!(now <= 8.0 + 1e-6, "it overshot the zoom: {now}");
        previous = now;
        frames += 1;
    }
    assert!(
        (camera.actual_distance() - 8.0).abs() < 1e-6,
        "{}",
        camera.actual_distance()
    );
    assert!(frames > 10 && frames < 180, "recovery took {frames} frames");
}

#[test]
fn a_returning_wall_pulls_the_camera_in_at_once() {
    // Easing is for going out. Coming in, a camera that eased would spend the easing inside
    // the wall.
    let walled = world_with(&[(Vec3::new(-10.0, 0.0, 3.0), Vec3::new(10.0, 6.0, 4.0))]);
    let open = world_with(&[]);
    let mut camera = camera_at(&open, Vec3::ZERO, 0.0, -0.22, 8.0);
    camera.follow(Vec3::ZERO, FRAME);
    camera.avoid_obstructions(&walled, Vec3::ZERO, FRAME);
    assert_valid(&walled, &camera, Vec3::ZERO, "the wall came back");
    assert!(camera.eye().z < 3.0);
}

#[test]
fn the_wheel_still_zooms_instantly_when_nothing_is_in_the_way() {
    let world = world_with(&[]);
    let mut camera = camera_at(&world, Vec3::ZERO, 0.0, -0.22, 8.0);
    camera.zoom(-3.0);
    camera.follow(Vec3::ZERO, FRAME);
    camera.avoid_obstructions(&world, Vec3::ZERO, FRAME);
    assert!((camera.actual_distance() - camera.distance).abs() < 1e-6);
    assert!(camera.distance > 8.0);
}

#[test]
fn a_wall_right_behind_the_character_clamps_the_camera_to_its_minimum_distance() {
    // Too close for the sphere to fit, far enough for the eye itself.
    let world = world_with(&[(Vec3::new(-10.0, 0.0, 0.5), Vec3::new(10.0, 6.0, 2.0))]);
    let camera = camera_at(&world, Vec3::ZERO, 0.0, -0.22, 8.0);
    let config = camera.collision;
    assert!(
        (camera.actual_distance() - config.min_distance).abs() < 1e-4,
        "{}",
        camera.actual_distance()
    );
    assert!(
        camera.eye().z < 0.5 - config.padding * 0.9,
        "{:?}",
        camera.eye()
    );
    assert_valid(&world, &camera, Vec3::ZERO, "minimum distance");

    // Closer still than the minimum: the eye goes no further than the wall allows. Never
    // through it, never behind the character to make room.
    let world = world_with(&[(Vec3::new(-10.0, 0.0, 0.25), Vec3::new(10.0, 6.0, 2.0))]);
    let camera = camera_at(&world, Vec3::ZERO, 0.0, -0.22, 8.0);
    assert!(camera.actual_distance() <= config.min_distance);
    assert!(camera.actual_distance() >= 0.0);
    assert!(camera.eye().z < 0.25, "{:?}", camera.eye());
    assert_valid(&world, &camera, Vec3::ZERO, "closer than the minimum");
}

#[test]
fn over_sloped_and_uneven_terrain_the_camera_stays_above_the_ground() {
    // A 64 m field that rises steeply along +Z (where the camera sits) and is rumpled by a
    // couple of metres either way.
    let resolution = 33;
    let heights = (0..resolution * resolution)
        .map(|index| {
            let (x, z) = ((index % resolution) as f32, (index / resolution) as f32);
            z * 0.9 + (x * 0.7).sin() * 1.2 + (z * 1.3).cos() * 0.8
        })
        .collect();
    let field =
        Heightfield::new(Vec3::new(-32.0, 0.0, -32.0), 2.0, resolution, heights).expect("a field");
    let mut world = CollisionWorld::new();
    world.add_static(StaticDesc::new(Shape::heightfield(field)).labelled("TERRAIN"));
    // A ramp beside the character too, so an oriented box is in the mix.
    world.add_static(
        StaticDesc::new(Shape::Obb(Obb::new(
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(1.5, 20.0, 4.0),
            Quat::from_axis_angle(Vec3::X, 0.5),
        )))
        .labelled("RAMP"),
    );
    world.rebuild_broad_phase();

    for (x, z) in [(-8.0, -8.0), (0.0, 0.0), (-4.0, 6.0), (8.0, -12.0)] {
        let ground = world
            .raycast(
                &Ray::new(Vec3::new(x, 200.0, z), -Vec3::Y).expect("a ray"),
                400.0,
                CollisionMask::WORLD,
            )
            .expect("the terrain is under the character");
        let character = ground.hit.point + Vec3::new(0.0, 0.05, 0.0);
        for step in 0..12 {
            let yaw = step as f32 * std::f32::consts::TAU / 12.0;
            for pitch in [-0.6, -0.1, 0.4, 1.0] {
                let camera = camera_at(&world, character, yaw, pitch, 14.0);
                let eye = camera.eye();
                let case = format!("terrain at ({x}, {z}), {yaw}/{pitch}");
                // Under a heightfield is the one place a ray going up still hits it.
                assert!(
                    world
                        .raycast(
                            &Ray::new(eye, Vec3::Y).expect("a ray"),
                            400.0,
                            CollisionMask::WORLD
                        )
                        .is_none_or(|hit| world.label(hit.id) != "TERRAIN"),
                    "{case}: the eye at {eye:?} is underneath the terrain"
                );
                assert_valid(&world, &camera, character, &case);
            }
        }
    }
}

#[test]
fn no_pitch_or_yaw_lets_the_camera_into_a_closed_room() {
    // A small room: the camera is boxed in from every side, at every angle the mouse can
    // reach, driven the way the mouse drives it.
    let world = world_with(&[
        (Vec3::new(-4.0, 0.0, -4.0), Vec3::new(-3.0, 4.0, 4.0)),
        (Vec3::new(3.0, 0.0, -4.0), Vec3::new(4.0, 4.0, 4.0)),
        (Vec3::new(-4.0, 0.0, -4.0), Vec3::new(4.0, 4.0, -3.0)),
        (Vec3::new(-4.0, 0.0, 3.0), Vec3::new(4.0, 4.0, 4.0)),
        (Vec3::new(-4.0, 3.2, -4.0), Vec3::new(4.0, 4.0, 4.0)),
    ]);
    let character = Vec3::new(1.0, 0.0, -0.5);
    let mut camera = camera_at(&world, character, 0.0, -0.22, 20.0);
    for frame in 0..600 {
        // A slow figure-eight with the mouse: yaw all the way round, pitch end to end.
        let t = frame as f32 / 600.0 * std::f32::consts::TAU;
        camera.rotate(t.cos() * 40.0, (2.0 * t).sin() * 60.0);
        camera.follow(character, FRAME);
        let (yaw, pitch) = (camera.yaw, camera.pitch);
        camera.avoid_obstructions(&world, character, FRAME);
        assert_eq!(
            (camera.yaw, camera.pitch),
            (yaw, pitch),
            "the check turned the camera"
        );
        assert_valid(&world, &camera, character, &format!("room, frame {frame}"));
    }
}

#[test]
fn standing_still_against_a_wall_the_camera_does_not_jitter() {
    let world = world_with(&[(Vec3::new(-10.0, 0.0, 1.2), Vec3::new(10.0, 6.0, 2.0))]);
    let mut camera = camera_at(&world, Vec3::ZERO, 0.3, -0.4, 8.0);
    let mut previous = camera.eye();
    for _ in 0..300 {
        camera.follow(Vec3::ZERO, FRAME);
        camera.avoid_obstructions(&world, Vec3::ZERO, FRAME);
        let eye = camera.eye();
        assert!((eye - previous).length() < 1e-5, "{previous:?} -> {eye:?}");
        previous = eye;
    }
}

#[test]
fn a_focus_that_lags_round_a_corner_does_not_drag_the_pivot_through_it() {
    // The focus is still beyond a pillar the character has just passed.
    let world = world_with(&[(Vec3::new(0.8, 0.0, -2.0), Vec3::new(1.2, 6.0, 2.0))]);
    let mut camera = ThirdPersonCamera::following(Vec3::new(2.0, 0.0, 0.0));
    camera.avoid_obstructions(&world, Vec3::ZERO, FRAME);
    assert!(
        camera.pivot().x < 0.8 - camera.collision.radius + 1e-3,
        "{:?}",
        camera.pivot()
    );
    assert_valid(&world, &camera, Vec3::ZERO, "lagging focus");
}

#[test]
fn a_teleport_places_the_camera_against_the_new_walls_at_once() {
    let world = world_with(&[(Vec3::new(40.0, 0.0, 3.0), Vec3::new(60.0, 6.0, 4.0))]);
    let mut camera = camera_at(&world, Vec3::ZERO, 0.0, -0.22, 8.0);
    let destination = Vec3::new(50.0, 0.0, 0.0);
    camera.snap_to(destination);
    camera.avoid_obstructions(&world, destination, 0.0);
    assert!(camera.eye().z < 3.0, "{:?}", camera.eye());
    assert_valid(&world, &camera, destination, "after a teleport");
}

// ---- fading ---------------------------------------------------------------------------------

#[test]
fn at_any_ordinary_zoom_the_character_is_drawn_whole() {
    // No floor either: looking up from below is a floor pulling the camera in, which is
    // exactly the case that fades — this one is about the zoom alone.
    let world = CollisionWorld::new();
    for distance in [2.5, 8.0, 25.0] {
        for pitch in [-1.2, -0.22, 0.5, 1.1] {
            let camera = camera_at(&world, Vec3::ZERO, 0.0, pitch, distance);
            assert_eq!(
                camera.character_opacity(),
                1.0,
                "zoom {distance}, pitch {pitch}"
            );
        }
    }
}

#[test]
fn a_wall_that_pushes_the_camera_in_fades_the_character_and_it_comes_back() {
    let walled = world_with(&[(Vec3::new(-10.0, 0.0, 1.0), Vec3::new(10.0, 6.0, 2.0))]);
    let open = world_with(&[]);
    let mut camera = camera_at(&walled, Vec3::ZERO, 0.0, -0.22, 8.0);
    let close = camera.character_opacity();
    assert!(
        close < 0.6,
        "the camera is up against the character and it is {close} drawn"
    );

    // Pressed right up against it, the character is gone rather than seen from inside.
    let boxed = world_with(&[(Vec3::new(-10.0, 0.0, 0.5), Vec3::new(10.0, 6.0, 2.0))]);
    let inside = camera_at(&boxed, Vec3::ZERO, 0.0, -0.22, 8.0);
    assert_eq!(inside.character_opacity(), 0.0);

    // The wall goes: the character comes back, eased rather than popping.
    camera.follow(Vec3::ZERO, FRAME);
    camera.avoid_obstructions(&open, Vec3::ZERO, FRAME);
    assert!(camera.character_opacity() < 0.95, "it popped back");
    for _ in 0..240 {
        camera.follow(Vec3::ZERO, FRAME);
        camera.avoid_obstructions(&open, Vec3::ZERO, FRAME);
    }
    // Exactly whole, not nearly: anything under the renderer's threshold keeps the character
    // on the two-pass see-through path, which is what halved the frame rate.
    assert_eq!(camera.character_opacity(), 1.0);
}

#[test]
fn the_fade_is_aimed_at_the_characters_chest_once_the_camera_has_looked() {
    let world = world_with(&[]);
    let unplaced = ThirdPersonCamera::following(Vec3::ZERO);
    assert_eq!(
        unplaced.fade(),
        None,
        "no fade before the camera knows where it is"
    );

    let character = Vec3::new(3.0, 0.5, -2.0);
    let camera = camera_at(&world, character, 0.4, -0.3, 8.0);
    let fade = camera.fade().expect("the check ran");
    assert!((fade.target - (character + Vec3::new(0.0, 1.4, 0.0))).length() < 1e-5);
    assert!(fade.near_start < fade.near_end);
    assert!(fade.radius > 0.0 && (0.0..1.0).contains(&fade.min_opacity));
}
