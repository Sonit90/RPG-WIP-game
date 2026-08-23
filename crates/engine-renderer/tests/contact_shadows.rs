//! The contact-shadow pass, in pixels.
//!
//! Contact shadows moved out of the mesh pixel shader into a pass of their own, at half
//! resolution, reconstructed with a depth-weighted filter. Three things have to stay true
//! through that move and none of them is checked by anything else:
//!
//! * the effect still darkens the contact region it exists for;
//! * it still only ever *darkens* — it must never invent light;
//! * half resolution still looks like full resolution.
//!
//! Everything renders through `render_scene_shadowed_debug` with the shadow-factor view, so
//! what is measured is the shadow term alone, with albedo, the sun's angle and the local
//! lights out of the way.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use std::sync::{Mutex, MutexGuard, OnceLock};

use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{Camera, Lighting, Projection, ShadowCasting};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::MeshData;
use engine_renderer::offscreen::{CapturedImage, ShadowSceneObject, render_scene_shadowed_debug};
use engine_renderer::shadow::{ShadowPreset, ShadowSettings};

fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn gpu_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, [0.0, 0.0, 0.0, 1.0]).is_ok()
}

const SIZE: u32 = 256;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The shadow-factor debug view: 1 is fully lit, 0 fully shadowed, and nothing else.
const SHADOW_FACTOR_VIEW: u32 = 2;

/// A sun low enough that a crate against the floor has a long contact seam.
fn low_sun() -> Lighting {
    Lighting {
        direction: Vec3::new(-2.0, -1.0, -0.6),
        color: Vec3::ONE,
        intensity: 1.0,
        ambient_color: Vec3::ONE,
        ambient_intensity: 0.1,
    }
}

/// Close to the floor and looking along it, which is where a contact shadow is read.
fn near_camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a square viewport");
    camera.set_position(Vec3::new(0.0, 1.6, 4.5));
    camera.set_rotation(Quat::from_yaw_pitch_roll(0.0, -0.32, 0.0));
    camera
}

/// The floor, and a scatter of crates resting on it.
///
/// Several rather than one, at several sizes: a contact shadow is a *seam*, and one crate
/// gives one seam whose position a threshold can be tuned around by accident.
fn objects<'a>(floor: &'a MeshData, crate_mesh: &'a MeshData) -> Vec<ShadowSceneObject<'a>> {
    let mut list = vec![ShadowSceneObject {
        transform: Transform::IDENTITY,
        mesh: floor,
        material: MaterialDesc::colored(0.8, 0.8, 0.8).with_roughness(1.0),
        shadow: ShadowCasting::RECEIVES_ONLY,
    }];
    for (x, z) in [(-1.2_f32, 0.0_f32), (0.0, -0.8), (1.3, 0.4)] {
        list.push(ShadowSceneObject {
            transform: Transform::from_translation(Vec3::new(x, 0.3, z)),
            mesh: crate_mesh,
            material: MaterialDesc::colored(0.7, 0.5, 0.3).with_roughness(1.0),
            shadow: ShadowCasting::CASTS_AND_RECEIVES,
        });
    }
    list
}

fn render(settings: ShadowSettings) -> CapturedImage {
    let floor = MeshData::plane(60.0);
    let crate_mesh = MeshData::cube(0.6);
    render_scene_shadowed_debug(
        SIZE,
        SIZE,
        CLEAR,
        &objects(&floor, &crate_mesh),
        &[],
        &near_camera(),
        low_sun(),
        settings,
        SHADOW_FACTOR_VIEW,
    )
    .expect("the shadowed scene must render")
}

/// The shadow factor at one pixel, 0..1.
fn factor(image: &CapturedImage, x: u32, y: u32) -> f32 {
    image
        .pixel(x, y)
        .map_or(0.0, |pixel| f32::from(pixel[0]) / 255.0)
}

/// Every pixel's factor, in row-major order.
fn factors(image: &CapturedImage) -> Vec<f32> {
    (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .map(|(x, y)| factor(image, x, y))
        .collect()
}

/// Ultra, with a **coarse** cascade and a long ray: the configuration that makes the effect
/// large enough to assert on.
///
/// Both departures from the shipping settings are deliberate and neither changes what is
/// being tested. 256 texels a cascade puts one texel at about 12 cm, so the centimetres where
/// a crate meets the floor are exactly what the shadow map cannot resolve — at the shipping
/// 2048 the cascade resolves that seam itself and the contact term has almost nothing left to
/// add, which is the right outcome for the game and a useless one for a test. And a 2 m ray
/// rather than 0.35 m makes the seam hundreds of pixels wide instead of tens, so a threshold
/// is measuring an effect rather than the edge of one.
///
/// Measured at the shipping settings for comparison: 12 pixels of a 256×256 frame differ, on
/// this scene, whether the march runs inline or as a pass. That is the size of the thing.
fn coarse_with_contact(enabled: bool, divisor: u32) -> ShadowSettings {
    let mut settings = ShadowPreset::Ultra.settings();
    settings.resolution = 256;
    settings.contact.enabled = enabled;
    settings.contact.divisor = divisor;
    settings.contact.length = 2.0;
    settings.contact.steps = 24;
    settings.validated()
}

#[test]
fn contact_shadows_darken_the_seam_where_a_crate_meets_the_floor() {
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    let without = factors(&render(coarse_with_contact(false, 2)));
    let with = factors(&render(coarse_with_contact(true, 2)));

    let darkened = with
        .iter()
        .zip(&without)
        .filter(|(after, before)| **before - **after > 0.05)
        .count();
    assert!(
        darkened > 300,
        "contact shadows darkened only {darkened} pixels of {}, which is not a contact seam",
        SIZE * SIZE
    );
}

#[test]
fn contact_shadows_never_brighten_anything() {
    // The effect multiplies into the shadow factor and is clamped to it; a pixel that got
    // *lighter* would mean the reconstruction is reading a neighbour it should have rejected
    // and inventing light out of it, which is the failure mode a bilinear upsample has.
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    let without = factors(&render(coarse_with_contact(false, 2)));
    let with = factors(&render(coarse_with_contact(true, 2)));

    let brightened = with
        .iter()
        .zip(&without)
        .filter(|(after, before)| **after - **before > 0.02)
        .count();
    assert_eq!(
        brightened, 0,
        "contact shadows made {brightened} pixels brighter than they were without them"
    );
}

#[test]
fn half_resolution_looks_like_full_resolution() {
    // The whole claim of the milestone. Half resolution is allowed to differ — it is a
    // quarter of the rays — but the *picture* has to be the same picture: the mean
    // difference over the frame is what a viewer would read as "it got worse", and a
    // handful of texels along a silhouette is what they would not.
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    let full = factors(&render(coarse_with_contact(true, 1)));
    let half = factors(&render(coarse_with_contact(true, 2)));

    let total: f32 = full
        .iter()
        .zip(&half)
        .map(|(a, b)| (a - b).abs())
        .sum::<f32>();
    let mean = total / (SIZE * SIZE) as f32;
    assert!(
        mean < 0.02,
        "half resolution differs from full by {mean:.4} of the shadow factor on average"
    );

    // And it must still be doing the work: a half-resolution pass that returned 1 everywhere
    // would pass the test above with a mean of nothing.
    let without = factors(&render(coarse_with_contact(false, 2)));
    let darkened = half
        .iter()
        .zip(&without)
        .filter(|(after, before)| **before - **after > 0.05)
        .count();
    assert!(darkened > 300, "the half-resolution pass darkened nothing");
}

#[test]
fn the_target_is_a_quarter_of_the_pixels_at_half_resolution() {
    // The setting means what it says. A divisor that quietly did nothing would leave every
    // measurement in the milestone report describing the wrong configuration.
    let settings = coarse_with_contact(true, 2);
    assert_eq!(settings.contact.target_size(3840, 2160), (1920, 1080));
    assert_eq!(settings.contact.target_size(1280, 720), (640, 360));
    // Odd sizes round up, so the last column has somewhere to be reconstructed from.
    assert_eq!(settings.contact.target_size(1281, 721), (641, 361));
    assert_eq!(
        coarse_with_contact(true, 1).contact.target_size(1920, 1080),
        (1920, 1080)
    );
}
