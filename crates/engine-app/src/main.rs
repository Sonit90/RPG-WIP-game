//! Game executable entry point.
//!
//! Wiring only: parse arguments, build the application, hand it to the engine.
//! The lifecycle and the frame loop belong to `engine-core`.

mod alloc_count;
mod app;
mod args;
mod audio_device;
mod benchmark;
mod map_bake;
mod run_profile;
mod startup;

use std::process::ExitCode;

use engine_core::application::{Engine, EngineConfig};

use crate::app::App;

fn main() -> ExitCode {
    // First statement in the process: everything after it is measured against this instant.
    startup::begin();
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

    // Configuration is validated before anything is created, so a bad value is a message
    // here rather than a failed Win32 call three layers down.
    let settings = args.to_settings();
    if let Err(error) = settings.validate() {
        eprintln!("error: {}\n\n{}", error.chain(), args::USAGE);
        return ExitCode::FAILURE;
    }

    startup::stage("process entry");

    let config = EngineConfig::default().with_frame_budget(settings.frame_budget);
    let result = App::new(&settings, &args).and_then(|app| Engine::new(app, config).run());

    // Records are written by a background thread, so every exit path drains before
    // returning: a process that exits with the queue full prints nothing about why.
    match result {
        Ok(()) => {
            log::info!("clean shutdown");
            engine_core::logging::drain();
            ExitCode::SUCCESS
        }
        Err(error) => {
            // The chain, not just the outermost message: "graphics: initialize the
            // renderer" alone says nothing about which call returned which HRESULT.
            let chain = error.chain();
            log::error!("fatal: {chain}");
            engine_core::logging::drain();
            eprintln!("fatal: {chain}");
            ExitCode::FAILURE
        }
    }
}
