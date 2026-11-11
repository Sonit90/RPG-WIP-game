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
//!
//! Milestone 4 changed what those figures are. Textures are deduplicated by content, so a
//! library that names the same bark image from twelve materials uploads it once and spends
//! one descriptor on it. The assertion below therefore counts *distinct* textures, and the
//! saving is asserted rather than merely printed: a regression that broke deduplication
//! would otherwise show up only as a number in a log that nobody diffs.

#![cfg(windows)]
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
    let baseline_textures = renderer.texture_count();
    let baseline_materials = renderer.material_count();
    println!(
        "startup: {} of {} persistent descriptors ({:.1}%) — the shared fallbacks, the \
         UI atlas and the shadow stage's own views; {baseline_textures} distinct texture(s) \
         from {} request(s)",
        baseline.used,
        baseline.capacity,
        baseline.utilization() * 100.0,
        renderer.texture_requests()
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
    let unique = (renderer.texture_count() - baseline_textures) as u32;
    // Identical materials are shared as well, so the table count is the distinct ones.
    let distinct_materials = (renderer.material_count() - baseline_materials) as u32;
    println!(
        "{materials} material request(s) resolved to {distinct_materials} distinct material(s)"
    );
    println!(
        "the environment library: {materials} materials, {textures} texture reference(s) \
         resolving to {unique} distinct image(s), {spent} descriptors — {} of {} used \
         ({:.1}%)",
        after.used,
        after.capacity,
        after.utilization() * 100.0
    );
    println!(
        "deduplication saved {} upload(s) and {} descriptor(s)",
        textures - unique,
        textures - unique
    );
    let (resident, naive) = renderer.texture_memory();
    println!(
        "texture memory: {} KB resident against {} KB for one uncompressed copy per          reference ({:.0}% saved)",
        resident / 1024,
        naive / 1024,
        100.0 - (resident as f64 / naive as f64) * 100.0
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
    // descriptor per *distinct* image plus MATERIAL_TEXTURE_COUNT per *distinct* material.
    assert!(distinct_materials <= materials);
    assert_eq!(
        spent,
        unique + distinct_materials * MATERIAL_TEXTURE_COUNT,
        "a scene costs one descriptor per distinct texture and {MATERIAL_TEXTURE_COUNT} per \
         material"
    );

    // And deduplication actually happened on this content. The environment library shares
    // images across its assets by construction — the same ground, the same trim, the same
    // packed ORM map — so a build where every reference produced its own upload would spend
    // strictly more here, and this is the assertion that notices.
    // And the memory that saving is made of. Deduplication alone accounts for it on this
    // content — the library's textures are small enough that block compression declines them
    // — so the assertion is against the naive figure rather than a compression ratio.
    assert!(
        resident < naive,
        "sharing images must cost less memory than one copy per reference: {resident}          against {naive}"
    );
    assert!(
        unique < textures,
        "the library shares images between materials, so {textures} reference(s) must \
         resolve to fewer than {textures} uploads, got {unique}"
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

/// The editor's terrain is rebuilt on every frame of a brush stroke. Each rebuild used to be
/// a new model — a new material, four more persistent descriptors — until a few seconds of
/// flattening filled the region and the ground could not be uploaded at all. Both ways of
/// rebuilding must now cost no descriptors after the first upload.
#[test]
fn rebuilding_run_time_geometry_spends_no_descriptors() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("terrain rebuild budget");
    let mut terrain = engine_environment::terrain::Terrain::flat(33, 2.0, 0.0);
    let model = engine_assets::procedural::terrain::terrain_model(&terrain, "grass");
    let binding = renderer.create_model(&model).expect("the first upload");
    let after_first = renderer.descriptor_stats().used;
    let materials = renderer.material_count();

    for stroke in 0..300 {
        terrain.apply_brush(
            engine_core::math::Vec3::new(stroke as f32 * 0.1, 0.0, 0.0),
            6.0,
            0.05,
            engine_environment::terrain::BrushMode::Raise,
        );
        let model = engine_assets::procedural::terrain::terrain_model(&terrain, "grass");
        renderer
            .replace_model_geometry(&binding, &model)
            .expect("an in-place rebuild");
    }
    // And the old way, a whole model per rebuild: identical materials are shared now.
    for _ in 0..50 {
        let model = engine_assets::procedural::terrain::terrain_model(&terrain, "grass");
        renderer.create_model(&model).expect("a fresh upload");
    }
    assert_eq!(
        renderer.descriptor_stats().used,
        after_first,
        "rebuilds spent descriptors"
    );
    assert_eq!(
        renderer.material_count(),
        materials,
        "identical materials were not shared"
    );
}
