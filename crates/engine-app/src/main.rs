//! Game executable entry point.
//!
//! Phase 0 / Milestone 1: opens a native window and pumps its message loop until the
//! user closes it. No device, no rendering — those arrive in Milestone 2 and 3.

use std::process::ExitCode;

use engine_platform::{PlatformError, Window, WindowConfig, WindowEvent};

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    log::info!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));

    match run() {
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

fn run() -> Result<(), PlatformError> {
    engine_platform::dpi::set_process_dpi_aware();

    let config = WindowConfig::new("myGame — phase 0", 1920, 1080);
    let mut window = Window::new(&config)?;

    let mut running = true;
    while running {
        window.pump_events();

        while let Some(event) = window.poll_event() {
            match event {
                WindowEvent::CloseRequested => {
                    log::info!("close requested");
                    running = false;
                }
                WindowEvent::Resized { width, height } => {
                    log::info!("resized to {width}x{height}");
                }
                WindowEvent::Minimized => log::info!("minimized"),
                WindowEvent::Restored => log::info!("restored"),
                WindowEvent::FocusChanged(focused) => log::debug!("focus: {focused}"),
            }
        }

        if window.quit_received() {
            running = false;
        }

        // Milestone 1 has no renderer, so without this the loop would spin a core at
        // 100% for no reason. Milestone 3 replaces it with the real frame pipeline and
        // no sleep at all.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    Ok(())
}
