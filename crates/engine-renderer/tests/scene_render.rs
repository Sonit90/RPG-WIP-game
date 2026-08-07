//! The 3D scene, judged by pixels.
//!
//! `RenderWorld` bookkeeping is unit-tested without a GPU. What needs a driver is that a
//! transform, a camera and a material together put the right colour in the right place —
//! the three things that are individually testable and only wrong in combination.
//!
//! These use the same pipeline, constant buffers and depth state as `Renderer::render_world`,
//! so a failure here is a failure of the path the window runs.

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

const SIZE: u32 = 128;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A camera on +Z looking back at the origin.
fn camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a valid projection");
    camera.set_position(Vec3::new(0.0, 0.0, 5.0));
    camera
}

fn center(image: &CapturedImage) -> [u8; 4] {
    image.pixel(SIZE / 2, SIZE / 2).expect("centre pixel")
}

fn red() -> MaterialDesc {
    MaterialDesc::colored(1.0, 0.0, 0.0)
}

fn green() -> MaterialDesc {
    MaterialDesc::colored(0.0, 1.0, 0.0)
}

#[test]
fn a_cube_in_front_of_the_camera_is_visible_and_its_material_colours_it() {
    let _guard = gpu_lock();

    let cube = MeshData::cube(2.0);
    let image = render_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[SceneObject {
            mesh: &cube,
            material: red(),
            transform: Transform::IDENTITY,
        }],
        &camera(),
        Lighting::default(),
    )
    .expect("render must succeed");

    let pixel = center(&image);
    assert!(
        pixel[0] > 80 && pixel[1] < 40,
        "expected the material's red, got {pixel:?}"
    );
    // The cube must not fill the frame: if the projection were wrong, it would either cover
    // everything or nothing, and both would still pass a "is the centre red" test.
    let background = image.count_pixels(|pixel| pixel[0] < 40 && pixel[1] < 40);
    assert!(
        background > 0,
        "the cube covered the whole target — the projection is wrong"
    );
}

#[test]
fn an_object_moved_out_of_view_is_not_drawn() {
    let _guard = gpu_lock();

    // The transform is the only difference from the test above, so a blank image here means
    // the transform reached the shader.
    let cube = MeshData::cube(2.0);
    let image = render_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[SceneObject {
            mesh: &cube,
            material: red(),
            transform: Transform::from_translation(Vec3::new(100.0, 0.0, 0.0)),
        }],
        &camera(),
        Lighting::default(),
    )
    .expect("render must succeed");

    assert_eq!(center(&image), [0, 0, 0, 255]);
    assert_eq!(image.count_pixels(|pixel| pixel[0] > 40), 0);
}

#[test]
fn the_nearer_of_two_objects_occludes_the_other() {
    let _guard = gpu_lock();

    // Submitted far-first and near-first: the depth buffer must decide, not the order.
    let cube = MeshData::cube(2.0);
    let near = SceneObject {
        mesh: &cube,
        material: red(),
        transform: Transform::from_translation(Vec3::new(0.0, 0.0, 1.0)),
    };
    let far = SceneObject {
        mesh: &cube,
        material: green(),
        transform: Transform::from_translation(Vec3::new(0.0, 0.0, -1.0)),
    };

    let a = render_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[near, far],
        &camera(),
        Lighting::default(),
    )
    .expect("render");
    let near = SceneObject {
        mesh: &cube,
        material: red(),
        transform: Transform::from_translation(Vec3::new(0.0, 0.0, 1.0)),
    };
    let far = SceneObject {
        mesh: &cube,
        material: green(),
        transform: Transform::from_translation(Vec3::new(0.0, 0.0, -1.0)),
    };
    let b = render_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[far, near],
        &camera(),
        Lighting::default(),
    )
    .expect("render");

    assert_eq!(center(&a), center(&b), "submission order changed the image");
    let pixel = center(&a);
    assert!(
        pixel[0] > 80 && pixel[1] < 40,
        "expected the nearer red cube, got {pixel:?}"
    );
}

#[test]
fn a_more_distant_object_covers_fewer_pixels() {
    let _guard = gpu_lock();

    // Perspective, not orthographic: this is the test that fails if the projection matrix
    // loses its w row and everything becomes the same size regardless of distance.
    let cube = MeshData::cube(1.0);
    let coverage = |z: f32| {
        let image = render_scene(
            SIZE,
            SIZE,
            CLEAR,
            &[SceneObject {
                mesh: &cube,
                material: red(),
                transform: Transform::from_translation(Vec3::new(0.0, 0.0, z)),
            }],
            &camera(),
            Lighting::default(),
        )
        .expect("render must succeed");
        image.count_pixels(|pixel| pixel[0] > 40)
    };

    let near = coverage(2.0);
    let far = coverage(-4.0);
    assert!(near > 0 && far > 0, "both cubes must be visible");
    assert!(
        near > far * 2,
        "a nearer cube must cover far more pixels: near {near}, far {far}"
    );
}

#[test]
fn each_object_gets_its_own_material() {
    let _guard = gpu_lock();

    // Side by side, so one pixel of each is unambiguous. If the material root CBV were
    // bound once instead of per draw, both would be the same colour.
    let cube = MeshData::cube(1.0);
    let image = render_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[
            SceneObject {
                mesh: &cube,
                material: red(),
                transform: Transform::from_translation(Vec3::new(-1.2, 0.0, 0.0)),
            },
            SceneObject {
                mesh: &cube,
                material: green(),
                transform: Transform::from_translation(Vec3::new(1.2, 0.0, 0.0)),
            },
        ],
        &camera(),
        Lighting::default(),
    )
    .expect("render must succeed");

    let reds = image.count_pixels(|pixel| pixel[0] > 80 && pixel[1] < 40);
    let greens = image.count_pixels(|pixel| pixel[1] > 80 && pixel[0] < 40);
    assert!(reds > 0, "the red cube is missing");
    assert!(greens > 0, "the green cube is missing");
}

#[test]
fn an_empty_scene_renders_the_clear_colour() {
    let _guard = gpu_lock();

    let image = render_scene(
        SIZE,
        SIZE,
        [0.1, 0.2, 0.3, 1.0],
        &[],
        &camera(),
        Lighting::default(),
    )
    .expect("render must succeed");
    let pixel = center(&image);
    // sRGB is not applied — the back buffer is UNORM — so the byte is the linear value.
    assert!(pixel[2] > pixel[1] && pixel[1] > pixel[0], "{pixel:?}");
}
