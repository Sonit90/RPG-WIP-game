//! Game executable entry point.
//!
//! Phase 0 / Milestone 0: bootstrap only. No window, no device, no rendering.
//! Later milestones add the platform window (M1) and the D3D12 renderer (M2+).

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    log::info!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));
    log::info!("phase 0 / milestone 0: bootstrap only, no graphics yet");
    log::debug!("debug logging is enabled");

    ExitCode::SUCCESS
}
