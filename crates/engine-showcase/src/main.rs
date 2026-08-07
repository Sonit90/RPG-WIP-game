//! Development-only visual test bed for the renderer.
//!
//! Not part of the game. Nothing depends on this crate, and a shipping build never links
//! it: it exists so a developer can *see* the renderer — camera, perspective, depth,
//! meshes, transforms, materials, directional lighting and frustum culling — rather than
//! only read that the tests pass.
//!
//! Everything on screen goes through the real engine: `engine_platform::Window`,
//! `engine_renderer::Renderer::render_world`, `RenderWorld`, `Camera`, `MeshData`,
//! `MaterialDesc` and the engine loop in `engine_core::application`. There is no separate
//! rendering path for the demo, which is what makes the picture evidence about the engine.
//!
//! Run it with `cargo run -p engine-showcase --release`; `--help` lists the options and the
//! controls are logged at startup.

mod app;
mod args;
mod camera;
mod geometry;
mod hud;
mod scene;

use std::process::ExitCode;

use engine_core::application::{Engine, EngineConfig};

use crate::app::Showcase;

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    let args = match args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("error: {error}\n\n{}", args::USAGE);
            return ExitCode::FAILURE;
        }
    };

    if args.help {
        println!("{}", args::USAGE);
        return ExitCode::SUCCESS;
    }

    log::info!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));

    let config = EngineConfig::default().with_frame_budget(args.frames);
    let result = Showcase::new(&args).and_then(|showcase| Engine::new(showcase, config).run());

    match result {
        Ok(()) => {
            log::info!("clean shutdown");
            ExitCode::SUCCESS
        }
        Err(error) => {
            // The chain, not just the outermost message: "graphics: initialize the renderer"
            // alone says nothing about which call returned which HRESULT.
            let chain = error.chain();
            log::error!("fatal: {chain}");
            eprintln!("fatal: {chain}");
            ExitCode::FAILURE
        }
    }
}
