//! Clustered local lighting, in pixels.
//!
//! The unit tests in `light_cluster.rs` assert that the grid contains what it should. These
//! assert that the *shader* reads the same grid the CPU wrote: a cluster lookup that is off
//! by a tile, a row inverted, or a depth slice computed with a different logarithm all
//! produce a grid that is perfectly correct and a picture where the light is somewhere else.
//!
//! Every test renders through `render_scene_shadowed`, which builds the same cluster grid the
//! window does from the same camera — so a passing test here is evidence about the path the
//! game takes.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use std::sync::{Mutex, MutexGuard, OnceLock};

use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{Camera, Lighting, LocalLight, LocalLightKind, Projection, ShadowCasting};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::MeshData;
use engine_renderer::offscreen::{CapturedImage, ShadowSceneObject, render_scene_shadowed};
use engine_renderer::shadow::{ShadowPreset, ShadowSettings};

/// One device at a time: several of these render at once otherwise, and a second adapter
/// open on the same queue is a source of flakiness rather than of coverage.
fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn gpu_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, [0.0, 0.0, 0.0, 1.0]).is_ok()
}

const SIZE: u32 = 192;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Almost no sun and almost no ambient.
///
/// The local lights are the whole signal here: with an ordinary sun every patch of floor is
/// bright and the difference this file measures is a few units out of two hundred.
fn dim() -> Lighting {
    Lighting {
        direction: Vec3::new(0.0, -1.0, 0.0),
        color: Vec3::ONE,
        intensity: 0.0,
        ambient_color: Vec3::ONE,
        ambient_intensity: 0.02,
    }
}

/// Looking straight down at the floor from 12 m, so a screen position is a world position.
fn top_down_camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a square viewport");
    camera.set_position(Vec3::new(0.0, 12.0, 0.0));
    camera.set_rotation(Quat::from_yaw_pitch_roll(
        0.0,
        -std::f32::consts::FRAC_PI_2,
        0.0,
    ));
    camera
}

/// A white point light at `position`, reaching `range` metres.
fn lamp(position: Vec3, range: f32) -> LocalLight {
    LocalLight {
        kind: LocalLightKind::Point,
        position,
        color: Vec3::ONE,
        intensity: 8.0,
        range,
        ..LocalLight::default()
    }
}

/// Mean brightness of a 4×4 patch, so one texel cannot decide a test.
fn patch(image: &CapturedImage, x: u32, y: u32) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for offset_y in 0..4 {
        for offset_x in 0..4 {
            if let Some(pixel) = image.pixel(x + offset_x, y + offset_y) {
                total += f32::from(pixel[0]) + f32::from(pixel[1]) + f32::from(pixel[2]);
                count += 3.0;
            }
        }
    }
    if count == 0.0 { 0.0 } else { total / count }
}

/// Renders a flat floor lit only by `lights`.
fn render(lights: &[LocalLight], settings: ShadowSettings) -> CapturedImage {
    let floor = MeshData::plane(40.0);
    let objects = vec![ShadowSceneObject {
        transform: Transform::IDENTITY,
        mesh: &floor,
        material: MaterialDesc::colored(0.8, 0.8, 0.8).with_roughness(1.0),
        shadow: ShadowCasting::RECEIVES_ONLY,
    }];
    render_scene_shadowed(
        SIZE,
        SIZE,
        CLEAR,
        &objects,
        lights,
        &top_down_camera(),
        dim(),
        settings,
    )
    .expect("the scene must render")
}

/// The floor spans about ±6.9 m at this camera, so one pixel is roughly 7 cm.
///
/// Screen x grows with world x and screen y grows with world **−z**, which is what the two
/// conversions below encode. Getting the second one backwards is exactly the bug the pixel
/// tests exist to catch, so it is written once here rather than at every call site.
fn screen_of(x: f32, z: f32) -> (u32, u32) {
    let half_extent = 12.0 * (std::f32::consts::FRAC_PI_6).tan();
    let to_pixel = |value: f32| {
        let normalized = value / half_extent * 0.5 + 0.5;
        (normalized * SIZE as f32)
            .round()
            .clamp(0.0, (SIZE - 5) as f32) as u32
    };
    (to_pixel(x), to_pixel(-z))
}

#[test]
fn a_local_light_brightens_the_floor_under_it() {
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    let settings = ShadowPreset::High.settings();
    let dark = render(&[], settings);
    let lit = render(&[lamp(Vec3::new(0.0, 2.0, 0.0), 6.0)], settings);

    let (x, y) = screen_of(0.0, 0.0);
    assert!(
        patch(&lit, x, y) > patch(&dark, x, y) + 20.0,
        "the floor under a lamp is no brighter than the floor with no lamp: {} vs {}",
        patch(&lit, x, y),
        patch(&dark, x, y)
    );
}

#[test]
fn a_light_does_not_reach_across_the_floor() {
    // The invariant the whole structure exists for, stated in pixels: a light in one part of
    // the view must not contribute to a pixel it cannot reach. It was already true — the
    // unclustered loop rejected the light on distance — and it must stay true now that the
    // rejection happens per cluster instead.
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    let settings = ShadowPreset::High.settings();
    let dark = render(&[], settings);
    let lit = render(&[lamp(Vec3::new(-5.0, 2.0, 0.0), 3.0)], settings);

    let (near_x, near_y) = screen_of(-5.0, 0.0);
    let (far_x, far_y) = screen_of(5.0, 0.0);
    assert!(
        patch(&lit, near_x, near_y) > patch(&dark, near_x, near_y) + 20.0,
        "the lamp does not light its own patch of floor"
    );
    assert!(
        (patch(&lit, far_x, far_y) - patch(&dark, far_x, far_y)).abs() < 2.0,
        "a 3 m lamp ten metres away changed the far side of the floor"
    );
}

#[test]
fn every_light_of_a_crowd_lights_its_own_patch() {
    // Sixteen lights in a grid. Each has to light the floor beneath it — which is the
    // assertion that fails if the mask is built for one light and read for another, or if
    // the bit for light thirteen is dropped somewhere between the CPU and the shader.
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    let settings = ShadowPreset::High.settings();
    let mut lights = Vec::new();
    let mut places = Vec::new();
    for row in 0..4 {
        for column in 0..4 {
            let x = -4.5 + 3.0 * column as f32;
            let z = -4.5 + 3.0 * row as f32;
            lights.push(lamp(Vec3::new(x, 1.5, z), 2.5));
            places.push((x, z));
        }
    }

    let dark = render(&[], settings);
    let lit = render(&lights, settings);
    for (x, z) in places {
        let (pixel_x, pixel_y) = screen_of(x, z);
        assert!(
            patch(&lit, pixel_x, pixel_y) > patch(&dark, pixel_x, pixel_y) + 20.0,
            "the lamp at ({x}, {z}) lights nothing"
        );
    }
}

/// A camera near the floor looking along it, so the picture spans many depth slices.
fn horizon_camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a square viewport");
    camera.set_position(Vec3::new(0.0, 2.0, 18.0));
    camera.set_rotation(Quat::from_yaw_pitch_roll(0.0, -0.16, 0.0));
    camera
}

#[test]
fn a_light_lights_its_own_depth_and_not_another() {
    // The depth half of the lookup. Looking down the floor, a near patch and a far patch are
    // in the same screen tile and in very different depth slices — so a shader that ignored
    // the slice, or computed it with a different logarithm than the CPU, would light the
    // wrong one. Nothing in the top-down tests above can catch that: they see one slice.
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    let settings = ShadowPreset::High.settings();
    let floor = MeshData::plane(80.0);
    let objects = vec![ShadowSceneObject {
        transform: Transform::IDENTITY,
        mesh: &floor,
        material: MaterialDesc::colored(0.8, 0.8, 0.8).with_roughness(1.0),
        shadow: ShadowCasting::RECEIVES_ONLY,
    }];
    let render_with = |lights: &[LocalLight]| {
        render_scene_shadowed(
            SIZE,
            SIZE,
            CLEAR,
            &objects,
            lights,
            &horizon_camera(),
            dim(),
            settings,
        )
        .expect("the scene must render")
    };

    let dark = render_with(&[]);
    // Two lamps down the centre line of the floor, one close to the camera and one far. Both
    // are in the middle column of the screen; only their depth tells them apart.
    let near = render_with(&[lamp(Vec3::new(0.0, 1.0, 6.0), 4.0)]);
    let far = render_with(&[lamp(Vec3::new(0.0, 1.0, -24.0), 8.0)]);

    let column = SIZE / 2 - 2;
    let brightest_row = |image: &CapturedImage| {
        (0..SIZE - 4)
            .step_by(4)
            .map(|row| (row, patch(image, column, row) - patch(&dark, column, row)))
            .fold(
                (0_u32, f32::MIN),
                |best, next| {
                    if next.1 > best.1 { next } else { best }
                },
            )
    };

    let (near_row, near_gain) = brightest_row(&near);
    let (far_row, far_gain) = brightest_row(&far);
    assert!(
        near_gain > 20.0,
        "the near lamp lights nothing: {near_gain}"
    );
    assert!(far_gain > 5.0, "the far lamp lights nothing: {far_gain}");
    // Further away is higher up the screen with this camera, so the far lamp's bright band
    // has to be above the near one's.
    assert!(
        far_row < near_row,
        "the far lamp lit row {far_row} and the near lamp row {near_row}, which is the wrong \
         way round"
    );
}
