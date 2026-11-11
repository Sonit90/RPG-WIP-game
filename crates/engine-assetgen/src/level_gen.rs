//! `level_gen`: write the showcase level into `assets/scenes/`.
//!
//! ```text
//!  cargo run -p engine-assetgen --bin level_gen
//!  cargo run -p engine-assetgen --bin level_gen -- --root some/other/assets
//! ```
//!
//! The counterpart of `asset_gen`: that one writes the starter *assets*, this one writes the
//! starter *level* built out of them. Both exist so that the generated content in the
//! repository can be regenerated rather than maintained by hand, and neither is linked into
//! the game — what ships is the file each of them writes.
//!
//! See [`engine_assetgen::showcase`] for what is in the level and why.

use std::path::PathBuf;

fn main() {
    let _ = engine_core::logging::init();

    let mut root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--root" => {
                let Some(value) = args.next() else {
                    log::error!("--root needs a directory");
                    std::process::exit(2);
                };
                root = PathBuf::from(value);
            }
            "--help" | "-h" => {
                println!("level_gen [--root <assets directory>]");
                return;
            }
            other => {
                log::error!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    let scene = engine_assetgen::showcase::showcase();
    let path = root.join("scenes").join("showcase.json");
    match engine_environment::scene_file::save(&scene, &path) {
        Ok(()) => {
            log::info!(
                "wrote {} - {} objects, {} collision shapes, {} water, {} spawn point(s)",
                path.display(),
                scene.objects.len(),
                scene.collision_shapes.len(),
                scene.water.len(),
                scene.spawn_points.len()
            );
        }
        Err(error) => {
            log::error!("could not write {}: {error}", path.display());
            std::process::exit(1);
        }
    }
}
