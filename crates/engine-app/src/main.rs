//! Game executable entry point.
//!
//! Phase 0 / Milestone 2: opens a window, initializes D3D12 and presents cleared frames
//! until the window is closed. Frame timing and statistics arrive in Milestone 3.

use std::fmt;
use std::process::ExitCode;

use engine_platform::{PlatformError, Window, WindowConfig, WindowEvent};
use engine_renderer::{Renderer, RendererConfig, RendererError};

/// Clear colour of the back buffer: a dark blue-grey, so an empty frame is obviously
/// rendered rather than an unpainted window.
const CLEAR_COLOR: [f32; 4] = [0.05, 0.07, 0.10, 1.0];

/// Anything that can end the run loop.
#[derive(Debug)]
enum AppError {
    Platform(PlatformError),
    Renderer(RendererError),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Platform(error) => write!(f, "{error}"),
            Self::Renderer(error) => write!(f, "{error}"),
        }
    }
}

impl From<PlatformError> for AppError {
    fn from(error: PlatformError) -> Self {
        Self::Platform(error)
    }
}

impl From<RendererError> for AppError {
    fn from(error: RendererError) -> Self {
        Self::Renderer(error)
    }
}

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

fn run() -> Result<(), AppError> {
    engine_platform::dpi::set_process_dpi_aware();

    let window_config = WindowConfig::new("myGame — phase 0", 1920, 1080);
    let mut window = Window::new(&window_config)?;

    let (width, height) = window.client_size();
    let mut renderer = Renderer::new(window.raw_handle(), &RendererConfig::new(width, height))?;

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
                    renderer.resize(width, height)?;
                }
                WindowEvent::Minimized => log::info!("minimized"),
                WindowEvent::Restored => log::info!("restored"),
                WindowEvent::FocusChanged(focused) => log::debug!("focus: {focused}"),
            }
        }

        if window.quit_received() {
            running = false;
        }

        // A minimized window has a zero-sized client area and nothing to present to.
        if !window.is_minimized() {
            renderer.render_clear(CLEAR_COLOR)?;
        }
    }

    log::info!("presented {} frames", renderer.frames_presented());
    Ok(())
}
