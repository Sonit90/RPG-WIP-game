//! Game executable entry point.
//!
//! Wiring only: parse arguments, build the application, hand it to the engine.
//! The lifecycle and the frame loop belong to `engine-core`.

mod app;
mod args;

use std::process::ExitCode;

use engine_core::application::{Engine, EngineConfig};

use crate::app::App;

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
    let result = App::new(&args).and_then(|app| Engine::new(app, config).run());

    match result {
        Ok(()) => {
            log::info!("clean shutdown");
            ExitCode::SUCCESS
        }
        Err(error) => {
            // The chain, not just the outermost message: "graphics: initialize the
            // renderer" alone says nothing about which call returned which HRESULT.
            let chain = error.chain();
            log::error!("fatal: {chain}");
            eprintln!("fatal: {chain}");
            ExitCode::FAILURE
        }
    }
}
