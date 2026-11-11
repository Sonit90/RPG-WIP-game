//! Directional lighting, judged by pixels.
//!
//! Lighting is where a wrong normal, a wrong matrix and a wrong light direction all become
//! visible at once, and where each of them can look plausible on its own. These tests
//! compare renders against each other rather than against magic colours, so they say
//! something about the relationship between geometry and light rather than about a constant.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_core::math::Vec3;
use engine_core::transform::Transform;
use engine_render_data::Lighting;
use engine_render_data::{Camera, Projection};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::MeshData;
use engine_renderer::offscreen::{CapturedImage, SceneObject, render_scene};

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

const SIZE: u32 = 96;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a valid projection");
    camera.set_position(Vec3::new(0.0, 0.0, 4.0));
    camera
}

/// Renders one white cube under `lighting` and returns the centre pixel's red channel.
fn front_face_brightness(lighting: Lighting) -> u8 {
    let cube = MeshData::cube(2.0);
    let image = render_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[SceneObject {
            mesh: &cube,
            material: MaterialDesc::colored(1.0, 1.0, 1.0),
            transform: Transform::IDENTITY,
        }],
        &camera(),
        lighting,
    )
    .expect("render must succeed");
    center(&image)[0]
}

fn center(image: &CapturedImage) -> [u8; 4] {
    image.pixel(SIZE / 2, SIZE / 2).expect("centre pixel")
}

/// A light travelling in `direction`, with no ambient so the diffuse term stands alone.
fn directional(direction: Vec3) -> Lighting {
    Lighting {
        direction,
        ambient_intensity: 0.0,
        ..Lighting::default()
    }
}

#[test]
fn a_surface_facing_the_light_is_brighter_than_one_facing_away() {
    let _guard = gpu_lock();

    // The camera sees the +Z face. A light travelling towards -Z hits it head on; one
    // travelling towards +Z hits the far side.
    let lit = front_face_brightness(directional(Vec3::new(0.0, 0.0, -1.0)));
    let unlit = front_face_brightness(directional(Vec3::new(0.0, 0.0, 1.0)));

    assert!(
        lit > 240,
        "a face square to the light should be near full: {lit}"
    );
    assert_eq!(
        unlit, 0,
        "a face turned away should get no diffuse: {unlit}"
    );
}

#[test]
fn brightness_follows_the_cosine_of_the_angle() {
    let _guard = gpu_lock();

    // Lambert: the diffuse term is dot(normal, to_light). At 45 degrees that is about
    // 0.707, so the face should be markedly dimmer than head-on but far from black. A
    // shader that used the raw dot without normalizing, or that lit by distance instead of
    // angle, fails this even though it passes the on/off test above.
    let head_on = f32::from(front_face_brightness(directional(Vec3::new(
        0.0, 0.0, -1.0,
    ))));
    let angled = f32::from(front_face_brightness(directional(Vec3::new(
        1.0, 0.0, -1.0,
    ))));

    let ratio = angled / head_on;
    assert!(
        (ratio - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.05,
        "expected cos(45 degrees), got {ratio}"
    );
}

#[test]
fn ambient_light_keeps_an_unlit_face_visible() {
    let _guard = gpu_lock();

    // Without ambient, a single directional light renders half of every object pure black:
    // there is no bounce lighting to fill it in.
    let away = Vec3::new(0.0, 0.0, 1.0);
    let dark = front_face_brightness(directional(away));
    let filled = front_face_brightness(Lighting {
        direction: away,
        ambient_intensity: 0.25,
        ..Lighting::default()
    });

    assert_eq!(dark, 0);
    assert!(
        (i16::from(filled) - 64).abs() <= 2,
        "0.25 ambient on white should be about 64, got {filled}"
    );
}

#[test]
fn light_colour_tints_the_surface() {
    let _guard = gpu_lock();

    let cube = MeshData::cube(2.0);
    let image = render_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[SceneObject {
            mesh: &cube,
            material: MaterialDesc::colored(1.0, 1.0, 1.0),
            transform: Transform::IDENTITY,
        }],
        &camera(),
        Lighting {
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: Vec3::new(1.0, 0.0, 0.0),
            ambient_intensity: 0.0,
            ..Lighting::default()
        },
    )
    .expect("render must succeed");

    let pixel = center(&image);
    assert!(
        pixel[0] > 240 && pixel[1] == 0 && pixel[2] == 0,
        "{pixel:?}"
    );
}

#[test]
fn rotating_an_object_changes_which_of_its_faces_is_lit() {
    let _guard = gpu_lock();

    // The normal is transformed by the model matrix. If it were left in local space, this
    // would render identically to the unrotated cube — which is the classic bug that only
    // lighting exposes.
    let cube = MeshData::cube(2.0);
    let brightness = |transform: Transform| {
        let image = render_scene(
            SIZE,
            SIZE,
            CLEAR,
            &[SceneObject {
                mesh: &cube,
                material: MaterialDesc::colored(1.0, 1.0, 1.0),
                transform,
            }],
            &camera(),
            // Lit from the right, so turning the cube swings its lit face away.
            directional(Vec3::new(-1.0, 0.0, 0.0)),
        )
        .expect("render must succeed");
        center(&image)[0]
    };

    let square_on = brightness(Transform::IDENTITY);
    let turned = brightness(Transform::from_rotation(
        engine_core::math::Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_4),
    ));

    assert_eq!(square_on, 0, "the +Z face is edge-on to a light from +X");
    assert!(
        turned > 100,
        "turning the cube should bring a lit face into view, got {turned}"
    );
}

#[test]
fn emissive_material_ignores_the_light() {
    let _guard = gpu_lock();

    let cube = MeshData::cube(2.0);
    let image = render_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[SceneObject {
            mesh: &cube,
            material: MaterialDesc::colored(0.0, 0.0, 0.0).with_emissive(Vec3::new(0.0, 1.0, 0.0)),
            transform: Transform::IDENTITY,
        }],
        &camera(),
        // Lit from behind, so anything visible came from the emissive term.
        directional(Vec3::new(0.0, 0.0, 1.0)),
    )
    .expect("render must succeed");

    let pixel = center(&image);
    assert!(pixel[1] > 240 && pixel[0] == 0, "{pixel:?}");
}
