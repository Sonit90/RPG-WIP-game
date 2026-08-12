//! Environment benchmark: placement, level-of-detail selection, extraction, culling and
//! batching, at eight scene configurations.
//!
//! No device, no window, no draws — the same arrangement `game_bench` uses, and for the same
//! reason:
//!
//! ```text
//! place -> update_lod -> extract -> cull -> group | record -> D3D12 -> present
//! ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ ^^^^^^^^^^^^^^^^^^^^^^^^^^^
//! this benchmark                                  not measured here
//! ```
//!
//! GPU frame time, FPS and the swap chain are properties of the whole frame, and they come
//! from running the game itself (`mygame --scene environment --frames <n>`). Mixing the two
//! measures presentation, which Milestone 7 found costs three to four times what recording
//! does — it would drown everything this benchmark exists to report.
//!
//! What *is* reported here and is a GPU-side number is the **draw-call and triangle count
//! the environment submits**, because both are decided entirely on the CPU: a draw call is a
//! `(mesh, material)` batch that survived the cull, and the triangles are the ones those
//! batches name. Counting them without a device is not an approximation of what the GPU is
//! asked to do; it is exactly what the GPU is asked to do.
//!
//! ```text
//! cargo run --release --bin env_bench
//! cargo run --release --bin env_bench -- --frames 2000
//! ```
//!
//! The process counts every heap allocation while measuring. "Placing the environment once
//! means it costs nothing per frame" is exactly the kind of claim that quietly stops being
//! true.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use engine_assets::procedural::{DummyVariant, environment_library, training_dummy};
use engine_core::math::{Frustum, Vec3};
use engine_environment::{EnvironmentScene, arena as env_arena};
use engine_game::arena as game_arena;
use engine_game::command::PlayerCommand;
use engine_game::sim::Simulation;
use engine_presentation::camera::ThirdPersonCamera;
use engine_presentation::environment::{EnvironmentPlacer, EnvironmentVisuals};
use engine_presentation::extract::{RenderExtractor, ViewParams, VisualBindings};
use engine_render_data::{
    Camera, MaterialHandle, MeshHandle, ModelBinding, ModelLodBinding, ModelPart, PartGeometry,
    Projection,
};
use engine_renderer::Culler;
use engine_renderer::render_list::RenderList;

/// Measured frames per scenario.
const DEFAULT_FRAMES: usize = 2_000;

/// Unmeasured frames before each scenario, so every buffer has reached its high-water mark.
const WARMUP_FRAMES: usize = 300;

/// Benchmark aspect ratio: 1920x1080, the stated target resolution.
const ASPECT: f32 = 1920.0 / 1080.0;

/// How aggressively levels are dropped. 1.0 is the thresholds the assets were authored with,
/// and the value the game runs at.
const LOD_BIAS: f32 = 1.0;

/// Counts every allocation the process makes.
///
/// A thin wrapper around the system allocator: the counter is a relaxed atomic add, which
/// costs far less than the allocation it is counting, so it does not distort what it
/// measures.
struct CountingAllocator;

/// Allocations since the process started.
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to the system allocator with the same arguments and returns
// its result unchanged, so the allocator contract is upheld by `System`. The counter is a
// side effect that touches no allocation state.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `layout` came from the caller and is forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` was returned by `alloc` with this `layout`.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `pointer` was returned by `alloc` with this `layout`, and `new_size` is
        // the caller's, forwarded unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Where the camera stands for a scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// The gameplay camera, following the player as it runs its rotation.
    Gameplay,
    /// A corner of the arena looking diagonally across the whole of it. The worst case for
    /// visible-object count, because almost nothing is behind the camera.
    Across,
    /// Standing in the middle looking at the gate: the arch, the gate pillars, the banners
    /// and the treeline behind them, which is the densest thing in the scene.
    Complex,
}

/// One scenario: a scene, a number of dummies and a camera.
struct Scenario {
    label: &'static str,
    scene: EnvironmentScene,
    dummies: usize,
    /// Whether gameplay entities are extracted at all. The empty-arena case is the
    /// environment on its own, which is the baseline everything else is measured against.
    gameplay: bool,
    view: View,
}

/// What one scenario measured.
struct Measurement {
    label: &'static str,
    objects: usize,
    dummies: usize,
    /// Render entities in the world: environment plus whatever extraction produced.
    drawables: usize,
    /// Microseconds per frame, by pass.
    lod: Stats,
    extraction: Stats,
    culling: Stats,
    grouping: Stats,
    /// The three together — what the environment and the fight cost the CPU each frame,
    /// before a single D3D12 call.
    total: Stats,
    visible: u32,
    culled: u32,
    draw_calls: usize,
    triangles: u64,
    lod_changes: usize,
    allocations: usize,
}

/// Mean and percentiles over a set of samples, in microseconds.
struct Stats {
    mean: f64,
    p50: f64,
    p99: f64,
    max: f64,
}

impl Stats {
    /// Summarizes `samples`, which is sorted in place.
    fn of(samples: &mut [f64]) -> Self {
        if samples.is_empty() {
            return Self {
                mean: 0.0,
                p50: 0.0,
                p99: 0.0,
                max: 0.0,
            };
        }
        let sum: f64 = samples.iter().sum();
        samples.sort_by(f64::total_cmp);
        Self {
            mean: sum / samples.len() as f64,
            p50: engine_core::time::percentile_of_sorted(samples, 0.50),
            p99: engine_core::time::percentile_of_sorted(samples, 0.99),
            max: samples[samples.len() - 1],
        }
    }
}

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    let mut frames = DEFAULT_FRAMES;
    let mut density = 4.0_f32;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--frames" => {
                let Some(value) = arguments.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("error: --frames needs a number");
                    return ExitCode::FAILURE;
                };
                frames = value;
            }
            "--density" => {
                let Some(value) = arguments.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("error: --density needs a number");
                    return ExitCode::FAILURE;
                };
                density = value;
            }
            "-h" | "--help" => {
                println!(
                    "usage: env_bench [--frames <count>] [--density <multiplier>]\n\n\
                     Measures the environment with no device: placement, level-of-detail\n\
                     selection, extraction, culling and batching, plus the draw-call and\n\
                     triangle counts the environment submits."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown option {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    match run(frames, density) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("fatal: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Runs every scenario and prints the report.
fn run(frames: usize, density: f32) -> Result<(), String> {
    let library = environment_library();
    let texture_bytes: usize = library.iter().map(|(_, model)| model.texture_bytes()).sum();
    let library_triangles: usize = library
        .iter()
        .map(|(_, model)| model.triangle_count())
        .sum();

    println!("=== ENVIRONMENT BENCHMARK ===");
    println!();
    println!("Mode: headless. No device, no window, no draws, no presentation.");
    println!("Resolution the frustum was built for: 1920x1080. LOD bias {LOD_BIAS}.");
    println!("Measured frames per scenario: {frames} (plus {WARMUP_FRAMES} warm-up)");
    println!(
        "Library: {} models, {library_triangles} triangles at LOD0, {:.1} KiB of textures",
        library.len(),
        texture_bytes as f64 / 1024.0
    );
    println!();

    let scenarios = vec![
        Scenario {
            label: "A empty arena",
            scene: env_arena::combat_arena(),
            dummies: 0,
            gameplay: false,
            view: View::Gameplay,
        },
        Scenario {
            label: "B + player",
            scene: env_arena::combat_arena(),
            dummies: 0,
            gameplay: true,
            view: View::Gameplay,
        },
        Scenario {
            label: "C + 10 dummies",
            scene: env_arena::combat_arena(),
            dummies: 10,
            gameplay: true,
            view: View::Gameplay,
        },
        Scenario {
            label: "D + 50 dummies",
            scene: env_arena::combat_arena(),
            dummies: 50,
            gameplay: true,
            view: View::Gameplay,
        },
        Scenario {
            label: "E + 100 dummies",
            scene: env_arena::combat_arena(),
            dummies: 100,
            gameplay: true,
            view: View::Gameplay,
        },
        Scenario {
            label: "F across arena",
            scene: env_arena::combat_arena(),
            dummies: 100,
            gameplay: true,
            view: View::Across,
        },
        Scenario {
            label: "G worst view",
            scene: env_arena::combat_arena(),
            dummies: 100,
            gameplay: true,
            view: View::Complex,
        },
        Scenario {
            label: "H stress",
            scene: env_arena::stress_arena(density),
            dummies: 10,
            gameplay: true,
            view: View::Across,
        },
    ];

    let mut results = Vec::new();
    for scenario in &scenarios {
        results.push(measure(scenario, frames)?);
    }

    println!(
        "{:<17} {:>7} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8} {:>7} {:>7} {:>9} {:>7}",
        "scenario",
        "objects",
        "drawn",
        "lod us",
        "extr us",
        "cull us",
        "group us",
        "total us",
        "p99 us",
        "visible",
        "culled",
        "triangles",
        "draws"
    );
    for result in &results {
        println!(
            "{:<17} {:>7} {:>7} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>8.2} {:>7} {:>7} \
             {:>9} {:>7}",
            result.label,
            result.objects,
            result.drawables,
            result.lod.mean,
            result.extraction.mean,
            result.culling.mean,
            result.grouping.mean,
            result.total.mean,
            result.total.p99,
            result.visible,
            result.culled,
            result.triangles,
            result.draw_calls
        );
    }
    println!();

    for result in &results {
        report(result, frames);
    }

    println!("A frame at 240 FPS is 4167 us; at 480 FPS it is 2083 us. The `total` column is");
    println!("the CPU cost of the environment and the fight before any D3D12 call, so it is");
    println!("a share of that budget rather than the whole of it. The `draws` column is the");
    println!("number of DrawIndexedInstanced calls the batches become, not the object count.");
    println!("Texture memory for the whole environment library: {texture_bytes} bytes.");
    Ok(())
}

/// Measures one scenario.
fn measure(scenario: &Scenario, frames: usize) -> Result<Measurement, String> {
    let bindings = benchmark_visuals();
    let mut simulation = game_arena::environment_arena(&scenario.scene, scenario.dummies);
    let mut extractor =
        RenderExtractor::new(bindings.gameplay.clone(), scenario.scene.half_extent * 2.0);
    // The scene brings its own ground, exactly as it does in the game.
    let ground = extractor.ground();
    extractor.render_world_mut().set_visible(ground, false);

    let mut placer = EnvironmentPlacer::new(bindings.environment.clone());
    let mut culler = Culler::new();
    let mut list = RenderList::new();
    let mut controller = ThirdPersonCamera::following(simulation.player_position());
    let mut camera = Camera::new(Projection::perspective(ASPECT))
        .map_err(|error| format!("the benchmark camera is invalid: {error}"))?;

    placer.place(
        extractor.render_world_mut(),
        &scenario.scene,
        simulation.player_position(),
        LOD_BIAS,
    );
    if placer.stats().unresolved > 0 {
        return Err(format!(
            "{} objects of '{}' name an asset that is not in the library",
            placer.stats().unresolved,
            scenario.scene.name
        ));
    }

    let (material_count, mesh_count) = (bindings.material_count, bindings.mesh_count);
    let mut frame = |index: usize,
                     extractor: &mut RenderExtractor,
                     placer: &mut EnvironmentPlacer,
                     simulation: &mut Simulation,
                     camera: &mut Camera,
                     controller: &mut ThirdPersonCamera|
     -> [f64; 4] {
        drive(simulation, index);
        simulation.step_once();
        place_camera(scenario.view, camera, controller, simulation);

        let started = Instant::now();
        placer.update_lod(extractor.render_world_mut(), camera.position(), LOD_BIAS);
        let lod = started.elapsed().as_secs_f64() * 1e6;

        let started = Instant::now();
        if scenario.gameplay {
            extractor.set_view(ViewParams {
                camera_position: camera.position(),
                delta_seconds: 1.0 / 240.0,
                lod_bias: LOD_BIAS,
            });
            extractor.extract(simulation);
        }
        let extraction = started.elapsed().as_secs_f64() * 1e6;

        let frustum = Frustum::from_view_projection(&camera.view_projection());
        let started = Instant::now();
        culler.cull(extractor.render_world(), &frustum, &mut list);
        let culling = started.elapsed().as_secs_f64() * 1e6;

        let started = Instant::now();
        // The same call `Renderer::build_render_list_with` makes, with the same counts the
        // renderer would pass: the lengths of its handle arrays.
        list.group_by_material_and_mesh(material_count, mesh_count);
        let grouping = started.elapsed().as_secs_f64() * 1e6;

        [lod, extraction, culling, grouping]
    };

    for index in 0..WARMUP_FRAMES {
        frame(
            index,
            &mut extractor,
            &mut placer,
            &mut simulation,
            &mut camera,
            &mut controller,
        );
    }

    let mut lod_samples = Vec::with_capacity(frames);
    let mut extraction_samples = Vec::with_capacity(frames);
    let mut culling_samples = Vec::with_capacity(frames);
    let mut grouping_samples = Vec::with_capacity(frames);
    let mut total_samples = Vec::with_capacity(frames);
    let mut lod_changes = 0;

    // Counted over the measured frames only: building the scene and warming up are expected
    // to allocate, and the question is whether the steady state does. The sample vectors are
    // reserved above so their growth is not attributed to a frame.
    let before = ALLOCATIONS.load(Ordering::Relaxed);

    for index in 0..frames {
        let [lod, extraction, culling, grouping] = frame(
            index + WARMUP_FRAMES,
            &mut extractor,
            &mut placer,
            &mut simulation,
            &mut camera,
            &mut controller,
        );
        lod_samples.push(lod);
        extraction_samples.push(extraction);
        culling_samples.push(culling);
        grouping_samples.push(grouping);
        total_samples.push(lod + extraction + culling + grouping);
        lod_changes += placer.stats().lod_changes;
    }

    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let triangles = submitted_triangles(&list, &bindings.triangles);

    Ok(Measurement {
        label: scenario.label,
        objects: scenario.scene.len(),
        dummies: scenario.dummies,
        drawables: extractor.render_world().len(),
        lod: Stats::of(&mut lod_samples),
        extraction: Stats::of(&mut extraction_samples),
        culling: Stats::of(&mut culling_samples),
        grouping: Stats::of(&mut grouping_samples),
        total: Stats::of(&mut total_samples),
        visible: culler.stats().visible,
        culled: culler.stats().culled,
        draw_calls: list.batches().len(),
        triangles,
        lod_changes,
        allocations,
    })
}

/// Triangles the surviving batches name.
///
/// Exactly what the GPU is asked to draw: for each batch, the triangles of its mesh times the
/// number of instances in it. Counted here rather than estimated, because "the environment
/// costs N triangles" is the sort of number that is quoted for years.
fn submitted_triangles(list: &RenderList, mesh_triangles: &HashMap<usize, u64>) -> u64 {
    list.batches()
        .iter()
        .map(|batch| {
            mesh_triangles
                .get(&batch.mesh.index())
                .copied()
                .unwrap_or(0)
                * u64::from(batch.count)
        })
        .sum()
}

/// Everything the benchmark draws, over handles nothing uploaded.
struct BenchBindings {
    environment: EnvironmentVisuals,
    /// What the extractor draws gameplay entities with.
    gameplay: VisualBindings,
    /// Triangles behind each mesh handle, for the submitted-triangle count.
    triangles: HashMap<usize, u64>,
    /// How many material and mesh handles were issued.
    ///
    /// The grouping pass is a counting sort over `material_count * mesh_count` buckets and
    /// **skips itself** above 64Ki of them, falling back to whatever runs the gather order
    /// already had. Passing generous round numbers here rather than the real counts is not a
    /// conservative choice — it silently turns the batching off and makes the benchmark
    /// report a draw call per object.
    material_count: usize,
    mesh_count: usize,
}

/// Hands out dense handle indices, exactly as the renderer does.
///
/// The renderer issues `MeshHandle::new(self.meshes.len() as u32, 1)` — the next free slot —
/// so handle indices are dense and both counts stay small. A benchmark that spaced its
/// handles out would produce a bucket array the grouping pass refuses to build, and would
/// then measure the un-batched fallback path while claiming to measure batching.
#[derive(Default)]
struct Handles {
    meshes: u32,
    materials: u32,
}

impl Handles {
    fn mesh(&mut self) -> MeshHandle {
        let handle = MeshHandle::new(self.meshes, 1);
        self.meshes += 1;
        handle
    }

    fn material(&mut self) -> MaterialHandle {
        let handle = MaterialHandle::new(self.materials, 1);
        self.materials += 1;
        handle
    }
}

/// Binds one model's levels and submeshes to fresh handles.
///
/// One mesh per (level, submesh) and one material per material index, which is what
/// `Renderer::create_model` produces — so two placements of one asset share both handles and
/// collapse into one batch, and two placements of two assets do not.
fn bind_model(
    model: &engine_assets::ModelAsset,
    handles: &mut Handles,
    triangles: &mut HashMap<usize, u64>,
) -> ModelBinding {
    let materials: Vec<MaterialHandle> = (0..model.materials.len())
        .map(|_| handles.material())
        .collect();
    let lods = model
        .lods
        .iter()
        .map(|lod| ModelLodBinding {
            parts: lod
                .submeshes
                .iter()
                .map(|submesh| {
                    let mesh = handles.mesh();
                    triangles.insert(mesh.index(), submesh.triangle_count() as u64);
                    ModelPart {
                        geometry: PartGeometry::Rigid(mesh),
                        material: materials
                            .get(submesh.material)
                            .copied()
                            .unwrap_or(MaterialHandle::new(0, 1)),
                        bounds: submesh.bounds,
                    }
                })
                .collect(),
            min_distance: lod.min_distance,
            bounds: lod.bounds(),
        })
        .collect();
    ModelBinding {
        lods,
        skinned: false,
        model_height: model
            .lod0()
            .map_or(1.0, |lod| lod.bounds().radius.max(0.5) * 2.0),
    }
}

/// The environment library and the gameplay bindings, over one dense handle space.
fn benchmark_visuals() -> BenchBindings {
    let mut handles = Handles::default();
    let mut triangles = HashMap::new();

    let mut environment = EnvironmentVisuals::new();
    for (asset, model) in environment_library() {
        environment.insert(asset, bind_model(&model, &mut handles, &mut triangles));
    }

    let single = training_dummy(DummyVariant::Single);
    let group = training_dummy(DummyVariant::Group);
    let dummy = bind_model(&single, &mut handles, &mut triangles);
    let dummy_group = bind_model(&group, &mut handles, &mut triangles);

    // The floor and the target marker the extractor owns. The floor is hidden in every
    // scenario — the scene brings its own — but it still holds a slot, exactly as it does in
    // the game.
    let unit = engine_core::math::Sphere::new(Vec3::ZERO, 0.87);
    let gameplay = VisualBindings {
        dummy,
        dummy_group,
        ground_mesh: handles.mesh(),
        ground_bounds: engine_core::math::Sphere::new(Vec3::ZERO, 1.0),
        ground_material: handles.material(),
        marker_mesh: handles.mesh(),
        marker_bounds: unit,
        marker_material: handles.material(),
    };

    BenchBindings {
        environment,
        gameplay,
        triangles,
        material_count: handles.materials as usize,
        mesh_count: handles.meshes as usize,
    }
}

/// Puts the camera where the scenario wants it.
fn place_camera(
    view: View,
    camera: &mut Camera,
    controller: &mut ThirdPersonCamera,
    simulation: &Simulation,
) {
    match view {
        View::Gameplay => {
            controller.follow(simulation.player_position(), 1.0 / 240.0);
            let _ = controller.apply(camera, ASPECT);
        }
        // From just inside one corner, looking at the opposite one. Almost the whole arena
        // and most of the treeline is in front of the camera, which is the case the culler
        // can do least about.
        View::Across => {
            let _ = camera.look_at(
                Vec3::new(-36.0, 6.0, 36.0),
                Vec3::new(20.0, 2.0, -30.0),
                Vec3::Y,
            );
        }
        // The gate: the arch, two pillars, two banners, the kerb behind them and the
        // treeline behind that. The densest thing in the scene by object count per pixel.
        View::Complex => {
            let _ = camera.look_at(
                Vec3::new(0.0, 4.0, 8.0),
                Vec3::new(0.0, 5.0, -60.0),
                Vec3::Y,
            );
        }
    }
}

/// Plays the same rotation on every scenario, so the scenes differ only in what is in them.
///
/// Deterministic and free of wall-clock time: the same frame index always issues the same
/// command, which is what makes two runs of the benchmark two measurements of one workload.
fn drive(simulation: &mut Simulation, frame: usize) {
    let angle = frame as f32 * 0.008;
    simulation.push_command(PlayerCommand::Move {
        forward: angle.cos(),
        right: angle.sin(),
        yaw: angle * 0.25,
        face_camera: false,
    });
    if frame % 90 == 0 {
        simulation.push_command(PlayerCommand::CycleTarget);
    }
}

/// Prints one scenario's detail block.
fn report(result: &Measurement, frames: usize) {
    println!("--- {} ---", result.label);
    println!(
        "  {} environment objects, {} dummies, {} render entities",
        result.objects, result.dummies, result.drawables
    );
    println!(
        "  visible {} / culled {} ({:.0}% rejected)",
        result.visible,
        result.culled,
        f64::from(result.culled) / f64::from(result.visible + result.culled).max(1.0) * 100.0
    );
    println!(
        "  {} draw calls, {} triangles submitted",
        result.draw_calls, result.triangles
    );
    for (name, stats) in [
        ("level of detail", &result.lod),
        ("extraction     ", &result.extraction),
        ("culling        ", &result.culling),
        ("grouping       ", &result.grouping),
        ("total          ", &result.total),
    ] {
        println!(
            "  {name}  mean {:>8.2}  p50 {:>8.2}  p99 {:>8.2}  max {:>8.2} us",
            stats.mean, stats.p50, stats.p99, stats.max
        );
    }
    println!(
        "  {} level changes over {frames} frames ({:.3} per frame)",
        result.lod_changes,
        result.lod_changes as f64 / frames as f64
    );
    println!(
        "  {} allocations over {frames} frames ({:.4} per frame)",
        result.allocations,
        result.allocations as f64 / frames as f64
    );
    println!();
}
