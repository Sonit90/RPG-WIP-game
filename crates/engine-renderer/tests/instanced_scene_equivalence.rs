//! One instanced draw per batch must produce the same image as one draw per object.
//!
//! This is the test §17 of the Phase 2 proposal asks for, and it exists because the
//! instanced path's two conventions are invisible on the CPU:
//!
//! * `INSTANCE_MODEL0`..`INSTANCE_MODEL3` in HLSL is one semantic with indices 0 to 3 — the
//!   trailing digits are the index, not part of the name.
//! * `float4x4(a, b, c, d)` fills rows while `Mat4` supplies columns, so the shader holds
//!   the transpose and must multiply vector-first.
//!
//! Getting either wrong leaves every counter reading correct — same draw count, same
//! triangle count, no validation error — and puts a completely different image on screen.
//! Only a pixel comparison catches that, so that is what this does.
//!
//! Renders offscreen: no window, no swap chain, no compositor, deterministic.

use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{Camera, Lighting, Projection};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::MeshData;
use engine_renderer::offscreen::{
    CapturedImage, SceneObject, render_scene, render_scene_instanced,
};

/// Offscreen target size. Large enough that a transform error moves many pixels, small
/// enough that the comparison is quick.
const WIDTH: u32 = 320;
/// Offscreen target height.
const HEIGHT: u32 = 240;

/// A dark clear colour, so any lit surface differs from the background.
const CLEAR: [f32; 4] = [0.02, 0.02, 0.04, 1.0];

fn camera() -> Camera {
    let aspect = WIDTH as f32 / HEIGHT as f32;
    let Ok(mut camera) = Camera::new(Projection::perspective(aspect)) else {
        panic!("a 4:3 perspective projection is valid");
    };
    let Ok(()) = camera.look_at(Vec3::new(0.0, 3.5, 9.0), Vec3::ZERO, Vec3::Y) else {
        panic!("the camera has somewhere to look");
    };
    camera
}

/// Pixels where the two images differ by more than `tolerance` in any channel.
///
/// A tolerance rather than exact equality: the two paths reach the same matrix by different
/// routes — a constant buffer against four vertex elements — so the last bit of a float can
/// legitimately differ. A transposed or mis-indexed matrix moves geometry by pixels rather
/// than by one least-significant bit, so this separates the two cleanly.
fn differing_pixels(left: &CapturedImage, right: &CapturedImage, tolerance: u8) -> usize {
    let mut differing = 0;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (Some(a), Some(b)) = (left.pixel(x, y), right.pixel(x, y)) else {
                panic!("both images cover {WIDTH}x{HEIGHT}");
            };
            if a.iter().zip(&b).any(|(l, r)| l.abs_diff(*r) > tolerance) {
                differing += 1;
            }
        }
    }
    differing
}

/// Renders `objects` both ways and asserts the images match.
fn assert_paths_agree(objects: &[SceneObject<'_>], what: &str) {
    let camera = camera();
    let lighting = Lighting::default();

    let (Ok(individual), Ok(instanced)) = (
        render_scene(WIDTH, HEIGHT, CLEAR, objects, &camera, lighting),
        render_scene_instanced(WIDTH, HEIGHT, CLEAR, objects, &camera, lighting),
    ) else {
        panic!("both offscreen paths must render {what}");
    };

    // The scene has to actually draw something, or two blank images would agree trivially.
    let lit = individual.count_pixels(|pixel| pixel[0] > 16 || pixel[1] > 16 || pixel[2] > 16);
    assert!(
        lit > 200,
        "{what}: the scene drew almost nothing ({lit} px)"
    );

    let differing = differing_pixels(&individual, &instanced, 1);
    assert_eq!(
        differing, 0,
        "{what}: {differing} pixels differ between the individual and instanced paths"
    );
}

#[test]
fn one_object_renders_the_same_either_way() {
    // The simplest case, and the one that isolates the matrix convention: a single instance
    // with a translation, a rotation and a non-unit scale.
    let cube = MeshData::cube(2.0);
    let objects = [SceneObject {
        mesh: &cube,
        material: MaterialDesc::colored(0.8, 0.3, 0.2),
        transform: Transform::from_translation(Vec3::new(1.0, 0.5, -1.0))
            .with_rotation(Quat::from_axis_angle(Vec3::Y, 0.6))
            .with_scale(Vec3::splat(1.4)),
    }];
    assert_paths_agree(&objects, "one transformed cube");
}

#[test]
fn a_batch_of_many_instances_renders_the_same_either_way() {
    // One batch, many instances. If the per-instance step rate were wrong, every instance
    // would read the first matrix and the whole batch would stack in one place.
    let cube = MeshData::cube(1.0);
    let material = MaterialDesc::colored(0.3, 0.7, 0.4);
    let objects: Vec<SceneObject<'_>> = (0..12)
        .map(|index| SceneObject {
            mesh: &cube,
            material,
            transform: Transform::from_translation(Vec3::new(
                (index % 4) as f32 * 2.0 - 3.0,
                (index / 4) as f32 * 1.6 - 1.6,
                0.0,
            ))
            .with_rotation(Quat::from_axis_angle(Vec3::Y, index as f32 * 0.3)),
        })
        .collect();
    assert_paths_agree(&objects, "twelve instances of one mesh");
}

#[test]
fn several_batches_render_the_same_either_way() {
    // Multiple meshes and materials, so the instanced path has to rebind between batches
    // and get each batch's instance range right.
    let cube = MeshData::cube(1.2);
    let sphere = MeshData::uv_sphere(0.7, 16, 10);
    let red = MaterialDesc::colored(0.85, 0.25, 0.2);
    let blue = MaterialDesc::colored(0.2, 0.4, 0.85);

    let mut objects = Vec::new();
    for index in 0..4 {
        objects.push(SceneObject {
            mesh: &cube,
            material: red,
            transform: Transform::from_translation(Vec3::new(index as f32 * 1.8 - 2.7, 1.2, 0.0)),
        });
    }
    for index in 0..4 {
        objects.push(SceneObject {
            mesh: &sphere,
            material: blue,
            transform: Transform::from_translation(Vec3::new(index as f32 * 1.8 - 2.7, -1.2, 0.0)),
        });
    }
    assert_paths_agree(&objects, "two batches of four");
}

#[test]
fn overlapping_depth_resolves_the_same_either_way() {
    // Depth is written by the instanced path exactly as by the individual one. If the two
    // disagreed about z, the near cube would stop occluding the far one.
    let cube = MeshData::cube(2.0);
    let objects = [
        SceneObject {
            mesh: &cube,
            material: MaterialDesc::colored(0.2, 0.8, 0.3),
            transform: Transform::from_translation(Vec3::new(0.0, 0.0, -3.0)),
        },
        SceneObject {
            mesh: &cube,
            material: MaterialDesc::colored(0.2, 0.8, 0.3),
            transform: Transform::from_translation(Vec3::new(0.3, 0.3, 1.0)),
        },
    ];
    assert_paths_agree(&objects, "two overlapping cubes");
}

#[test]
fn a_non_uniform_scale_renders_the_same_either_way() {
    // Non-uniform scale is where a transposed matrix stops being a rotation and starts
    // being a shear, so it is the case most likely to expose a convention error.
    let cube = MeshData::cube(1.0);
    let objects = [SceneObject {
        mesh: &cube,
        material: MaterialDesc::colored(0.9, 0.8, 0.2),
        transform: Transform::from_translation(Vec3::ZERO)
            .with_rotation(Quat::from_axis_angle(Vec3::new(1.0, 1.0, 0.0), 0.9))
            .with_scale(Vec3::new(3.0, 0.5, 1.5)),
    }];
    assert_paths_agree(&objects, "a sheared, non-uniformly scaled cube");
}
