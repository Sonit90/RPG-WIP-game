//! What each shadow filter actually produces.
//!
//! The 5×5 grid was replaced by a rotated Vogel disc of four or six taps. "Fewer taps" is
//! easy to measure and worthless on its own — a single tap is fewer taps — so what is
//! asserted here is the three properties the replacement was chosen for:
//!
//! * **penumbra width**, because a filter that is cheaper by being narrower is not cheaper;
//! * **stability**, because a per-pixel dither that is fixed in screen space is what crawls
//!   when the camera moves, and this engine has no temporal pass to hide it;
//! * **that every kernel still draws a shadow at all**.
//!
//! Everything renders through the shadow-factor debug view, so albedo and the sun's angle are
//! out of the way and what is measured is the filter.

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
use engine_renderer::offscreen::{ShadowSceneObject, render_scene_shadowed_debug};
use engine_renderer::shadow::{PcfKernel, ShadowPreset, ShadowSettings};

fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn gpu_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, [0.0, 0.0, 0.0, 1.0]).is_ok()
}

const SIZE: u32 = 384;
/// The shadow-factor debug view.
const SHADOW_FACTOR_VIEW: u32 = 2;

/// A cube two metres above a floor, lit by a sun tilted along +X so its shadow lands beside
/// it rather than under it. The same arrangement `shadow_render.rs` uses, and for the same
/// reason: with the light straight down there is no pixel that shows the shadow and not the
/// caster.
fn frame(kernel: PcfKernel, camera_shift: f32) -> Vec<f32> {
    let floor = MeshData::plane(60.0);
    let cube = MeshData::cube(1.5);
    let objects = vec![
        ShadowSceneObject {
            transform: Transform::IDENTITY,
            mesh: &floor,
            material: MaterialDesc::colored(0.8, 0.8, 0.8).with_roughness(1.0),
            shadow: ShadowCasting::RECEIVES_ONLY,
        },
        ShadowSceneObject {
            transform: Transform::from_translation(Vec3::new(0.0, 2.0, 0.0)),
            mesh: &cube,
            material: MaterialDesc::colored(0.9, 0.3, 0.2).with_roughness(1.0),
            shadow: ShadowCasting::CASTS_AND_RECEIVES,
        },
    ];

    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a square viewport");
    camera.set_position(Vec3::new(camera_shift, 12.0, 0.0));
    camera.set_rotation(Quat::from_yaw_pitch_roll(
        0.0,
        -std::f32::consts::FRAC_PI_2,
        0.0,
    ));

    let lighting = Lighting {
        direction: Vec3::new(-1.5, -1.0, 0.0),
        color: Vec3::ONE,
        intensity: 1.0,
        ambient_color: Vec3::ONE,
        ambient_intensity: 0.1,
    };

    let mut settings: ShadowSettings = ShadowPreset::Ultra.settings();
    settings.pcf = kernel;
    // Contact shadows off: they darken the same pixels and would be measured as filter width.
    settings.contact.enabled = false;

    let image = render_scene_shadowed_debug(
        SIZE,
        SIZE,
        [0.0, 0.0, 0.0, 1.0],
        &objects,
        &[],
        &camera,
        lighting,
        settings.validated(),
        SHADOW_FACTOR_VIEW,
    )
    .expect("the shadowed scene must render");

    (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .map(|(x, y)| {
            image
                .pixel(x, y)
                .map_or(0.0, |pixel| f32::from(pixel[0]) / 255.0)
        })
        .collect()
}

/// Whether a shadow factor is inside the penumbra rather than fully lit or fully dark.
fn partial(value: f32) -> bool {
    (0.05..0.95).contains(&value)
}

/// The median count of partly-shadowed pixels along a scanline: the penumbra's width.
fn penumbra_width(frame: &[f32]) -> usize {
    let mut widths: Vec<usize> = ((SIZE / 3)..(2 * SIZE / 3))
        .map(|y| {
            (0..SIZE)
                .filter(|x| partial(frame[(y * SIZE + x) as usize]))
                .count()
        })
        .filter(|width| *width > 0)
        .collect();
    widths.sort_unstable();
    widths.get(widths.len() / 2).copied().unwrap_or(0)
}

/// How much the penumbra changes when the camera moves one centimetre.
///
/// The number "crawling" is. A filter whose dither is fixed in screen space leaves its pattern
/// behind while the shadow slides under it; a smooth filter changes almost nothing.
fn drift(kernel: PcfKernel) -> f32 {
    let still = frame(kernel, 0.0);
    let moved = frame(kernel, 0.01);
    let mut total = 0.0;
    let mut count = 0.0;
    for (a, b) in still.iter().zip(&moved) {
        if partial(*a) || partial(*b) {
            total += (a - b).abs();
            count += 1.0;
        }
    }
    if count > 0.0 { total / count } else { 0.0 }
}

#[test]
fn every_kernel_draws_a_shadow() {
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    for kernel in PcfKernel::ALL {
        let shadowed = frame(kernel, 0.0)
            .iter()
            .filter(|value| **value < 0.5)
            .count();
        assert!(
            shadowed > 500,
            "{} shadowed only {shadowed} pixels",
            kernel.as_str()
        );
    }
}

#[test]
fn the_disc_kernels_are_no_narrower_than_the_grids_they_replace() {
    // The point of the replacement. Four taps on a disc must not be a *narrower* filter than
    // nine on a grid, or the saving is being taken out of the picture rather than out of the
    // fetch count.
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    let grid_3x3 = penumbra_width(&frame(PcfKernel::Tap3x3, 0.0));
    let grid_5x5 = penumbra_width(&frame(PcfKernel::Tap5x5, 0.0));
    let disc_4 = penumbra_width(&frame(PcfKernel::Poisson4, 0.0));
    let disc_6 = penumbra_width(&frame(PcfKernel::Poisson6, 0.0));

    assert!(
        disc_4 >= grid_3x3,
        "the four-tap disc is {disc_4} px wide against the 3x3's {grid_3x3}"
    );
    assert!(
        disc_6 >= grid_5x5,
        "the six-tap disc is {disc_6} px wide against the 5x5's {grid_5x5}"
    );
    assert!(
        disc_6 >= disc_4,
        "six taps produced a narrower penumbra than four"
    );
}

#[test]
fn the_disc_kernels_are_steadier_than_the_grid_they_replace() {
    // The property a stochastic filter has to earn, because it is the one it is most likely
    // to lose: this engine has no temporal accumulation, so the per-pixel rotation is a
    // function of the pixel alone and of nothing that changes per frame. If it were not, a
    // static shadow would boil.
    let _guard = gpu_lock();
    if !gpu_available() {
        eprintln!("skipped: no D3D12 device");
        return;
    }
    let grid_3x3 = drift(PcfKernel::Tap3x3);
    let grid_5x5 = drift(PcfKernel::Tap5x5);
    let disc_4 = drift(PcfKernel::Poisson4);
    let disc_6 = drift(PcfKernel::Poisson6);

    assert!(
        disc_4 < grid_3x3,
        "the four-tap disc drifts {disc_4:.4} against the 3x3's {grid_3x3:.4}"
    );
    assert!(
        disc_6 < disc_4,
        "six taps drift more than four: {disc_6:.4} against {disc_4:.4}"
    );
    // The 5x5 is twenty-five ordered taps and will always be the steadiest; the disc has to
    // be close to it, not better than it.
    assert!(
        disc_6 < grid_5x5 * 1.25,
        "the six-tap disc drifts {disc_6:.4}, more than a quarter above the 5x5's {grid_5x5:.4}"
    );
}

#[test]
fn the_presets_use_the_disc() {
    // A kernel nobody selects is a kernel nobody measured. This is what catches a preset
    // quietly left on the grid after the tuning work.
    assert_eq!(ShadowPreset::Ultra.settings().pcf, PcfKernel::Poisson6);
    assert_eq!(ShadowPreset::High.settings().pcf, PcfKernel::Poisson4);
    assert_eq!(ShadowPreset::Medium.settings().pcf, PcfKernel::Poisson4);
    assert_eq!(ShadowPreset::Low.settings().pcf, PcfKernel::Single);
    for preset in [
        ShadowPreset::Medium,
        ShadowPreset::High,
        ShadowPreset::Ultra,
    ] {
        let settings = preset.settings();
        assert!(settings.pcf.is_poisson());
        assert!(settings.pcf.taps() <= 8, "{} taps", settings.pcf.taps());
    }
}
