//! The application: window, renderer and the per-stage work the engine loop drives.
//!
//! The loop itself lives in `engine-core` ([`engine_core::application::Engine`]). This
//! module supplies the stages:
//!
//! ```text
//! process_events() -> update() -> render() -> present() -> end_frame()
//! ```
//!
//! Each stage is timed by the engine, so a future bottleneck can be attributed to a stage
//! rather than to "the frame". There is no frame cap and no sleep: the loop runs as fast
//! as the CPU and GPU allow, which is the only way the 240/360/480 FPS targets can be
//! measured honestly.
//!
//! Window and device creation happen in [`App::new`] rather than in
//! [`Application::initialize`]: both produce values the application cannot exist without,
//! and holding them as `Option` to allow a two-phase init would put a check on every use
//! for no gain.

use std::fmt;
use std::time::{Duration, Instant};

use engine_core::application::{Application, EngineContext};
use engine_core::metrics::FrameSummary;
use engine_platform::{Key, PlatformError, Window, WindowConfig, WindowEvent};
use engine_renderer::stress::{STRESS_LEVELS, level_at, nearest_level_index};
use engine_renderer::{Renderer, RendererConfig, RendererError};

use crate::args::Args;

/// Clear colour of the back buffer: a dark blue-grey, so an empty frame is obviously
/// rendered rather than an unpainted window.
const CLEAR_COLOR: [f32; 4] = [0.05, 0.07, 0.10, 1.0];

/// Anything that can end the run loop.
#[derive(Debug)]
pub enum AppError {
    /// The platform layer failed.
    Platform(PlatformError),
    /// The renderer failed.
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

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Platform(error) => Some(error),
            Self::Renderer(error) => Some(error),
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

/// Window, renderer and reporting state.
///
/// Frame-time history lives in `EngineContext::metrics`, not here: every application that
/// the engine drives gets the same statistics, computed the same way.
pub struct App {
    window: Window,
    renderer: Renderer,
    report_interval: Option<Duration>,
    last_report: Instant,
    /// Index into [`STRESS_LEVELS`] of the level currently loaded.
    stress_index: usize,
    /// Overlay text, rebuilt on an interval rather than per frame: formatting allocates.
    #[cfg(feature = "debug-overlay")]
    overlay_lines: Vec<String>,
    #[cfg(feature = "debug-overlay")]
    last_overlay_update: Instant,
}

/// Function key that toggles the debug overlay.
///
/// The overlay ships in release builds, so players and testers need a way to bring it up
/// without a command line.
#[cfg(feature = "debug-overlay")]
const OVERLAY_TOGGLE_KEY: u8 = 5;

/// How often the overlay text is rebuilt.
///
/// Four updates a second is readable without the numbers flickering, and it keeps string
/// formatting — which allocates — off the per-frame path.
#[cfg(feature = "debug-overlay")]
const OVERLAY_UPDATE_INTERVAL: Duration = Duration::from_millis(250);

impl App {
    /// Creates the window and the renderer.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] if the window or the D3D12 device cannot be created.
    pub fn new(args: &Args) -> Result<Self, AppError> {
        engine_platform::dpi::set_process_dpi_aware();

        let window_config = WindowConfig::new("myGame — phase 1", args.width, args.height);
        let window = Window::new(&window_config)?;

        let (width, height) = window.client_size();
        let renderer_config = RendererConfig::new(width, height).with_vsync(args.vsync);
        let mut renderer = Renderer::new(window.raw_handle(), &renderer_config)?;
        #[cfg(feature = "debug-overlay")]
        renderer.set_overlay_enabled(args.overlay);

        renderer.set_scene_triangles(args.triangles.max(1))?;

        let report_interval = if args.report_interval_secs > 0.0 {
            Some(Duration::from_secs_f64(args.report_interval_secs))
        } else {
            None
        };

        Ok(Self {
            window,
            renderer,
            report_interval,
            last_report: Instant::now(),
            stress_index: nearest_level_index(args.triangles.max(1)),
            #[cfg(feature = "debug-overlay")]
            overlay_lines: Vec::with_capacity(5),
            #[cfg(feature = "debug-overlay")]
            last_overlay_update: Instant::now(),
        })
    }

    /// Loads stress level `index` and resets the statistics windows.
    ///
    /// The old level's frame times say nothing about the new one, so keeping them would
    /// blend two workloads into one set of percentiles.
    fn set_stress_level(&mut self, index: usize, ctx: &mut EngineContext) -> Result<(), AppError> {
        let triangles = level_at(index);
        if triangles == self.renderer.scene_triangle_count() {
            return Ok(());
        }

        self.renderer.set_scene_triangles(triangles)?;
        self.stress_index = index;

        // Clears the frame-time history as well: the previous level's timings describe a
        // different workload.
        ctx.reset_clock();
        log::info!("stress level {}: {triangles} triangles", index + 1);
        Ok(())
    }

    /// Logs frame statistics if the reporting interval has elapsed.
    ///
    /// Logging allocates while formatting, so it happens on an interval and never per
    /// frame.
    fn maybe_report(&mut self, summary: FrameSummary) {
        let Some(interval) = self.report_interval else {
            return;
        };
        if self.last_report.elapsed() < interval {
            return;
        }
        self.last_report = Instant::now();

        let stats = summary.cpu;
        let stages = summary.stages;
        let (events, update, render, present) = (
            stages.events_ms,
            stages.update_ms,
            stages.render_ms,
            stages.present_ms,
        );

        log::info!(
            "CPU frame: mean {:.3} ms ({:.0} FPS) | p50 {:.3} | p95 {:.3} | p99 {:.3} \
             ({:.0} FPS) | min {:.3} | max {:.3} | n={}",
            stats.mean_ms,
            stats.mean_fps(),
            stats.p50_ms,
            stats.p95_ms,
            stats.p99_ms,
            stats.p99_fps(),
            stats.min_ms,
            stats.max_ms,
            stats.count
        );
        log::info!(
            "stages: events {events:.3} ms | update {update:.3} ms | \
             render {render:.3} ms | present {present:.3} ms | draws {} | tris {} | \
             scene {} tris",
            self.renderer.draw_calls_last_frame(),
            self.renderer.triangles_last_frame(),
            self.renderer.scene_triangle_count()
        );

        // GPU time comes from GPU timestamps, not wall clock, and lags the CPU numbers by
        // a few frames. It is reported on its own line so the two are never mistaken for
        // simultaneous samples of the same frame.
        if let Some(gpu) = summary.gpu {
            log::info!(
                "GPU frame: mean {:.3} ms ({:.0} FPS) | p50 {:.3} | p95 {:.3} | p99 {:.3} | \
                 min {:.3} | max {:.3} | n={}",
                gpu.mean_ms,
                gpu.mean_fps(),
                gpu.p50_ms,
                gpu.p95_ms,
                gpu.p99_ms,
                gpu.min_ms,
                gpu.max_ms,
                gpu.count
            );
        }
    }

    /// Rebuilds the overlay text if the update interval has elapsed.
    ///
    /// The strings are rebuilt in place, so after the first update this reuses the same
    /// `String` allocations rather than making new ones.
    #[cfg(feature = "debug-overlay")]
    fn maybe_update_overlay(&mut self, summary: FrameSummary) {
        use std::fmt::Write as _;

        if !self.renderer.overlay_enabled()
            || self.last_overlay_update.elapsed() < OVERLAY_UPDATE_INTERVAL
        {
            return;
        }
        self.last_overlay_update = Instant::now();

        let cpu = summary.cpu;
        let gpu = summary.gpu;
        let (width, height) = self.renderer.size();

        while self.overlay_lines.len() < 5 {
            self.overlay_lines.push(String::new());
        }

        self.overlay_lines[0].clear();
        let _ = write!(
            self.overlay_lines[0],
            "FPS {:.0}  ({:.0} P99)",
            cpu.mean_fps(),
            cpu.p99_fps()
        );

        self.overlay_lines[1].clear();
        let _ = write!(
            self.overlay_lines[1],
            "CPU {:.3} MS  P99 {:.3} MS",
            cpu.mean_ms, cpu.p99_ms
        );

        self.overlay_lines[2].clear();
        match gpu {
            Some(gpu) => {
                let _ = write!(
                    self.overlay_lines[2],
                    "GPU {:.3} MS  P99 {:.3} MS",
                    gpu.mean_ms, gpu.p99_ms
                );
            }
            None => {
                let _ = write!(self.overlay_lines[2], "GPU -- MS");
            }
        }

        self.overlay_lines[3].clear();
        let _ = write!(
            self.overlay_lines[3],
            "{} TRIS  {} DRAWS",
            self.renderer.scene_triangle_count(),
            self.renderer.draw_calls_last_frame()
        );

        self.overlay_lines[4].clear();
        let _ = write!(
            self.overlay_lines[4],
            "{}X{}  {}",
            width,
            height,
            self.renderer.adapter_info().name
        );

        self.renderer.set_overlay_text(&self.overlay_lines);
    }

    /// Logs a final summary over the last window of frames.
    fn report_final(&mut self, ctx: &mut EngineContext) {
        let frames = ctx.frame_index();
        let seconds = ctx.elapsed().as_secs_f64();
        log::info!(
            "ran {frames} frames in {seconds:.3} s ({:.0} FPS average over the whole run)",
            if seconds > 0.0 {
                frames as f64 / seconds
            } else {
                0.0
            }
        );
        // Recompute rather than reading the cached summary: the last refresh can be up to
        // a quarter of a second old, and the final numbers should cover the whole window.
        ctx.metrics_mut().refresh();
        let Some(summary) = ctx.metrics().summary() else {
            return;
        };
        log::info!(
            "final CPU window: mean {:.3} ms | p95 {:.3} ms | p99 {:.3} ms | n={}",
            summary.cpu.mean_ms,
            summary.cpu.p95_ms,
            summary.cpu.p99_ms,
            summary.cpu.count
        );
        if let Some(gpu) = summary.gpu {
            log::info!(
                "final GPU window: mean {:.3} ms | p95 {:.3} ms | p99 {:.3} ms | n={}",
                gpu.mean_ms,
                gpu.p95_ms,
                gpu.p99_ms,
                gpu.count
            );
        }
    }
}

impl Application for App {
    type Error = AppError;

    /// Drains window messages and applies them.
    fn process_events(&mut self, ctx: &mut EngineContext) -> Result<(), AppError> {
        self.window.pump_events();

        while let Some(event) = self.window.poll_event() {
            match event {
                WindowEvent::CloseRequested => {
                    log::info!("close requested");
                    ctx.request_exit();
                }
                WindowEvent::Resized { width, height } => {
                    log::info!("resized to {width}x{height}");
                    self.renderer.resize(width, height)?;
                }
                WindowEvent::Minimized => log::info!("minimized"),
                WindowEvent::Restored => log::info!("restored"),
                WindowEvent::FocusChanged(focused) => log::debug!("focus: {focused}"),
                // F1-F4 pick the first four stress levels, F6 the largest, F7/F8 step.
                // F5 is the overlay toggle, so the levels skip it.
                WindowEvent::KeyPressed(Key::Function(number @ 1..=4)) => {
                    self.set_stress_level(usize::from(number) - 1, ctx)?;
                }
                WindowEvent::KeyPressed(Key::Function(6)) => {
                    self.set_stress_level(STRESS_LEVELS.len() - 1, ctx)?;
                }
                WindowEvent::KeyPressed(Key::Function(7)) => {
                    let index = self.stress_index.saturating_sub(1);
                    self.set_stress_level(index, ctx)?;
                }
                WindowEvent::KeyPressed(Key::Function(8)) => {
                    let index = (self.stress_index + 1).min(STRESS_LEVELS.len() - 1);
                    self.set_stress_level(index, ctx)?;
                }
                #[cfg(feature = "debug-overlay")]
                WindowEvent::KeyPressed(Key::Function(OVERLAY_TOGGLE_KEY)) => {
                    let enabled = !self.renderer.overlay_enabled();
                    self.renderer.set_overlay_enabled(enabled);
                    log::info!("debug overlay {}", if enabled { "shown" } else { "hidden" });
                    if enabled {
                        // Force an immediate rebuild so the overlay appears on the next
                        // frame instead of at the next scheduled update.
                        self.last_overlay_update = Instant::now() - OVERLAY_UPDATE_INTERVAL;
                    }
                }
                WindowEvent::KeyPressed(key) => log::trace!("key pressed: {key:?}"),
            }
        }

        if self.window.quit_received() {
            ctx.request_exit();
        }
        Ok(())
    }

    /// Advances the simulation. Empty so far — there is no gameplay to simulate.
    fn update(&mut self, _ctx: &mut EngineContext) -> Result<(), AppError> {
        Ok(())
    }

    fn render(&mut self, _ctx: &mut EngineContext) -> Result<(), AppError> {
        self.renderer.render(CLEAR_COLOR)?;
        Ok(())
    }

    fn present(&mut self, _ctx: &mut EngineContext) -> Result<(), AppError> {
        self.renderer.present()?;
        Ok(())
    }

    /// A minimized window has a zero-sized client area and nothing to present to.
    fn is_render_ready(&self) -> bool {
        !self.window.is_minimized()
    }

    fn gpu_frame_ms(&self) -> Option<f64> {
        self.renderer.last_gpu_frame_ms()
    }

    /// Reporting and overlay text only; the engine has already recorded the frame.
    ///
    /// Both consumers read the cached summary, so neither triggers a percentile sort.
    fn end_frame(&mut self, ctx: &mut EngineContext) {
        let Some(summary) = ctx.metrics().summary() else {
            return;
        };
        self.maybe_report(summary);
        #[cfg(feature = "debug-overlay")]
        self.maybe_update_overlay(summary);
    }

    fn shutdown(&mut self, ctx: &mut EngineContext) {
        self.report_final(ctx);
    }
}
