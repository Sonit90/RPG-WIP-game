//! What the material texture set costs in descriptors.
//!
//! Milestone 2 took a material from one descriptor to four. That is an architectural cost
//! and not a regression, but "four per material" is only affordable if the heap is sized for
//! it — and the failure mode of getting it wrong is a named error at scene load, on a
//! machine that had enough room yesterday.
//!
//! This test measures the real figures against the real environment library rather than
//! asserting an arithmetic identity, and prints them so the numbers in the milestone report
//! come from a run rather than from a calculation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_platform::{Window, WindowConfig};
use engine_renderer::device::RenderDevice;
use engine_renderer::material::MATERIAL_TEXTURE_COUNT;
use engine_renderer::{Renderer, RendererConfig, RendererError};

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

fn gpu_available() -> bool {
    match RenderDevice::new(false, false) {
        Ok(_) => true,
        Err(RendererError::NoSuitableAdapter { examined }) => {
            eprintln!("SKIP: no D3D12 adapter on this machine ({examined} examined)");
            false
        }
        Err(error) => panic!("unexpected D3D12 initialization failure: {error}"),
    }
}

fn renderer_for(title: &str) -> (Window, Renderer) {
    let window = Window::new(&WindowConfig::new(title, 320, 240).with_visible(false))
        .expect("hidden window creation must succeed");
    let config = RendererConfig::new(320, 240)
        .with_debug_layer(true)
        .with_break_on_validation_error(false);
    let renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");
    (window, renderer)
}

#[test]
fn the_whole_environment_library_fits_in_the_persistent_region_with_room_left() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("descriptor budget");

    let baseline = renderer.descriptor_stats();
    println!(
        "startup: {} of {} persistent descriptors ({:.1}%) — the four shared fallbacks, the \
         UI atlas and the shadow stage's own views",
        baseline.used,
        baseline.capacity,
        baseline.utilization() * 100.0
    );

    let mut materials = 0_u32;
    let mut textures = 0_u32;
    let started = std::time::Instant::now();
    for (_asset, model) in engine_assets::procedural::environment::environment_library() {
        materials += model.materials.len() as u32;
        textures += model.textures.len() as u32;
        renderer
            .create_model(&model)
            .expect("every library asset uploads");
    }
    let upload = started.elapsed();

    let after = renderer.descriptor_stats();
    let spent = after.used - baseline.used;
    println!(
        "the environment library: {materials} materials, {textures} textures, {spent} \
         descriptors — {} of {} used ({:.1}%)",
        after.used,
        after.capacity,
        after.utilization() * 100.0
    );
    println!(
        "before M2 the same library would have spent {} descriptors ({materials} materials \
         at one each, plus {textures} textures)",
        materials + textures
    );

    // Load-time only, and dominated by the per-material constant-buffer upload's own GPU
    // wait rather than by the three extra `CreateShaderResourceView` calls M2 added. Printed
    // rather than asserted: it is a machine-dependent figure, and a threshold on it would
    // fail on a slower machine without anything having regressed.
    println!(
        "the whole library uploaded in {:.1} ms - {:.3} ms per material",
        upload.as_secs_f64() * 1000.0,
        upload.as_secs_f64() * 1000.0 / f64::from(materials)
    );

    // The architectural cost, stated as an assertion so it cannot drift silently: one
    // descriptor per uploaded texture plus MATERIAL_TEXTURE_COUNT per material.
    assert_eq!(
        spent,
        textures + materials * MATERIAL_TEXTURE_COUNT,
        "a scene costs one descriptor per texture and {MATERIAL_TEXTURE_COUNT} per material"
    );

    // Headroom, not a tight fit. The heap is 4096 x 32 bytes = 128 KB, so being generous
    // here costs nothing worth measuring, and running out is a named error at load rather
    // than corruption at draw.
    assert!(
        after.utilization() < 0.5,
        "the whole environment library must leave the persistent region at least half free, \
         used {} of {}",
        after.used,
        after.capacity
    );
    assert_eq!(after.failures, 0, "no allocation was refused");
}
