//! Dungeon map benchmark: the CPU half of the bake, and the per-frame cost of the minimap and
//! the full-screen map as the number of encounter markers grows.
//!
//! ```text
//! walkable_mask + style_map        once per bake        (the GPU capture is measured by the
//!                                                         game's own log: `map '...': baked`)
//! DungeonMap::push_minimap         every frame          markers, clip, one picture quad
//! DungeonMap::push_map_screen      every frame it is up markers, sidebar, text
//! UiBatcher::build                 every frame          the renderer's CPU side of the same
//! ```
//!
//! Deterministic: packs are laid on a grid from their index, so two runs measure the same
//! work. No device.
//!
//! ```text
//! cargo run --release --bin map_bench
//! cargo run --release --bin map_bench -- --iterations 2000
//! ```

use std::process::ExitCode;
use std::time::Instant;

use engine_core::math::Vec3;
use engine_environment::map::{frame_for, walkable_mask};
use engine_environment::{EnvironmentScene, MonsterPlacement, scene_file};
use engine_presentation::ui::dungeon_map::{
    DungeonMap, DungeonMapInfo, MapMarkers, MapTheme, MapUnit,
};
use engine_presentation::ui::input::UiInput;
use engine_render_data::ui::{UiDrawList, UiScale};
use engine_renderer::ui_pipeline::UiBatcher;

/// Percentiles of a set of samples, in microseconds.
fn summary(samples: &mut [f64]) -> (f64, f64, f64) {
    samples.sort_by(f64::total_cmp);
    let at = |fraction: f64| samples[((samples.len() - 1) as f64 * fraction) as usize];
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    (mean, at(0.5), at(0.99))
}

/// The crypt, or an empty level if it cannot be read.
fn crypt() -> EnvironmentScene {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenes/graveyard_crypt.json");
    scene_file::load(&path).unwrap_or_else(|error| {
        eprintln!("warning: {error}; using an empty level");
        EnvironmentScene::empty("empty")
    })
}

/// The crypt with `extra` more single-monster packs laid on a grid inside its frame.
fn crowded(base: &EnvironmentScene, extra: usize) -> EnvironmentScene {
    let mut scene = base.clone();
    let frame = frame_for(base);
    let side = (extra as f32).sqrt().ceil().max(1.0) as usize;
    for index in 0..extra {
        let (column, row) = (index % side, index / side);
        let x = frame.min_x + (column as f32 + 0.5) / side as f32 * frame.width();
        let z = frame.min_z + (row as f32 + 0.5) / side as f32 * frame.depth();
        scene.monsters.push(
            MonsterPlacement::new("crypt_ghoul", Vec3::new(x, 0.0, z))
                .in_pack(format!("STRESS {index}")),
        );
    }
    scene
}

fn main() -> ExitCode {
    let mut iterations = 1_000usize;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--iterations" => match arguments.next().and_then(|value| value.parse().ok()) {
                Some(value) => iterations = value,
                None => {
                    eprintln!("error: --iterations needs a number");
                    return ExitCode::FAILURE;
                }
            },
            other => {
                eprintln!("error: unknown option {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    let base = crypt();
    let frame = frame_for(&base);
    let (width, height) = frame.texture_size(base.map.texels_per_metre);

    // The bake's CPU half, at the crypt's real size.
    let mut mask_us = Vec::new();
    let mut style_us = Vec::new();
    let capture = vec![96u8; width as usize * height as usize * 4];
    for _ in 0..5 {
        let started = Instant::now();
        let mask = walkable_mask(&base, &frame, width, height);
        mask_us.push(started.elapsed().as_secs_f64() * 1e6);
        let started = Instant::now();
        let styled = engine_assets::map_image::style_map(
            &capture,
            (width, height),
            &mask,
            &engine_assets::map_image::MapPalette::default(),
        );
        style_us.push(started.elapsed().as_secs_f64() * 1e6);
        if styled.is_err() {
            eprintln!("error: styling failed");
            return ExitCode::FAILURE;
        }
    }
    let (mask_mean, _, _) = summary(&mut mask_us);
    let (style_mean, _, _) = summary(&mut style_us);
    println!(
        "bake CPU at {width}x{height}: walkable mask {:.1} ms, styling {:.1} ms",
        mask_mean / 1e3,
        style_mean / 1e3
    );
    println!(
        "picture memory: {:.1} MB RGBA8 with mips",
        (width as f64 * height as f64 * 4.0 * 4.0 / 3.0) / (1024.0 * 1024.0)
    );
    println!();
    println!(
        "{:>9} | {:>22} | {:>22} | {:>22}",
        "markers", "minimap us mean/p99", "map us mean/p99", "batch us mean (map)"
    );

    let viewport = (1920, 1080);
    let theme = MapTheme;
    let markers = MapMarkers {
        player: Some(MapUnit {
            position: Vec3::new(0.0, 0.0, -8.0),
            facing: Some(0.4),
        }),
        view_yaw: 0.7,
        party: vec![MapUnit {
            position: Vec3::new(3.0, 0.0, -10.0),
            facing: None,
        }],
        ..MapMarkers::default()
    };
    for extra in [0usize, 50, 200, 1000] {
        let scene = crowded(&base, extra);
        let mut map = DungeonMap::new(UiScale::new(1.0));
        map.set_info(Some(DungeonMapInfo::from_scene(&scene, frame, true)));
        let count = map.info().map_or(0, |info| info.encounters.len());
        let mut list = UiDrawList::with_capacity(4096, 65536);
        let mut batcher = UiBatcher::new(65536);

        let mut minimap = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            list.clear();
            let started = Instant::now();
            map.update(&UiInput::new(), 0.0, viewport, &markers);
            map.push_minimap(&mut list, &markers, viewport, &theme);
            minimap.push(started.elapsed().as_secs_f64() * 1e6);
        }

        map.open();
        map.select(Some(count.saturating_sub(extra + 1)));
        let mut screen = Vec::with_capacity(iterations);
        let mut batch = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            list.clear();
            let started = Instant::now();
            map.update(&UiInput::new(), 0.0, viewport, &markers);
            map.push_map_screen(&mut list, &markers, viewport, &theme);
            screen.push(started.elapsed().as_secs_f64() * 1e6);
            let started = Instant::now();
            let _ = batcher.build(&list);
            batch.push(started.elapsed().as_secs_f64() * 1e6);
        }
        let (mini_mean, _, mini_p99) = summary(&mut minimap);
        let (map_mean, _, map_p99) = summary(&mut screen);
        let (batch_mean, _, _) = summary(&mut batch);
        println!(
            "{count:>9} | {:>10.1} / {:>9.1} | {:>10.1} / {:>9.1} | {:>22.1}",
            mini_mean, mini_p99, map_mean, map_p99, batch_mean
        );
    }
    ExitCode::SUCCESS
}
