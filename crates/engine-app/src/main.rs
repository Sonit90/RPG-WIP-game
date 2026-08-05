//! Game executable entry point.
//!
//! Phase 0 / Milestone 3: window, D3D12 device and an uncapped frame loop with CPU
//! frame-time statistics. GPU frame time arrives in Milestone 5.

mod app;
mod args;

use std::process::ExitCode;

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

    let result = App::new(&args).and_then(|mut app| app.run());

    match result {
        Ok(()) => {
            log::info!("clean shutdown");
            ExitCode::SUCCESS
        }
        Err(error) => {
            log::error!("fatal: {error}");
            eprintln!("fatal: {error}");
            ExitCode::FAILURE
        }
    }
}
