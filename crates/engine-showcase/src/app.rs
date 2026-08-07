//! The showcase application: window, renderer, scene and the developer controls.
//!
//! Drives the same [`engine_core::application::Application`] stages the game binary does —
//! `process_events -> update -> render -> present -> end_frame` — so what the overlay reports
//! is measured by the engine loop, not by the showcase.

use std::time::{Duration, Instant};

use engine_core::application::{Application, EngineContext};
use engine_core::error::EngineError;
use engine_core::input::{InputEvent, KeyCode, MouseButton};
use engine_platform::{Window, WindowConfig, WindowEvent};
use engine_renderer::{Camera, Lighting, Projection, Renderer, RendererConfig};

use crate::args::Args;
use crate::camera::FlyCamera;
use crate::hud::{self, FrameGraph, GRAPH_BUDGET_MS, HudStats};
use crate::scene::{PRESETS, Scene};

/// Clear colour: a dark blue-grey that reads as sky without competing with the objects.
const CLEAR_COLOR: [f32; 4] = [0.07, 0.09, 0.13, 1.0];

/// How often the overlay text is rebuilt.
///
/// Formatting allocates, so it happens on an interval and never per frame. Ten a second is
/// responsive enough to watch a number move while you fly.
const OVERLAY_UPDATE_INTERVAL: Duration = Duration::from_millis(100);

/// Per-frame upload memory reserved by the showcase.
///
/// The stress preset submits one root constant-buffer view per visible object, each
/// 256-byte aligned. With culling switched off (F8), 100,000 objects need about 25 MiB in a
/// single frame — the default 4 MiB would fail the allocation and end the run. This is a
/// development tool, so it reserves the headroom rather than refusing the mode.
const UPLOAD_BYTES_PER_FRAME: usize = 40 * 1024 * 1024;

/// Window, renderer, scene and developer state.
pub struct Showcase {
    window: Window,
    renderer: Renderer,
    camera: Camera,
    controller: FlyCamera,
    scene: Scene,
    lighting: Lighting,
    /// Whether the mouse is captured and drives the view.
    mouse_captured: bool,
    /// Frame times behind the overlay's graph.
    graph: FrameGraph,
    overlay_lines: Vec<String>,
    last_overlay_update: Instant,
    /// Seconds of animation time, accumulated from the engine's delta.
    animation_seconds: f32,
    report_interval: Option<Duration>,
    last_report: Instant,
}

impl Showcase {
    /// Creates the window, the renderer and the scene.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the window, the D3D12 device or the scene's GPU resources
    /// cannot be created.
    pub fn new(args: &Args) -> Result<Self, EngineError> {
        // Before any window exists: without it a fullscreen window on a scaled display
        // covers only part of the screen, and the size the renderer is given does not match
        // the pixels on screen.
        engine_platform::dpi::set_process_dpi_aware();

        let window_config =
            WindowConfig::new("myGame — renderer showcase", args.width, args.height)
                .with_fullscreen(args.fullscreen);
        let window = Window::new(&window_config)?;

        // The window is the authority on its client size: fullscreen takes the monitor's,
        // and the OS may clamp a request on a small display.
        let (width, height) = window.client_size();
        let mut renderer_config = RendererConfig::new(width, height).with_vsync(args.vsync);
        renderer_config.upload_bytes_per_frame = UPLOAD_BYTES_PER_FRAME;
        renderer_config.frames_in_flight = args.frames_in_flight;
        renderer_config.enable_debug_layer = args.validation;
        renderer_config.break_on_validation_error = args.validation && cfg!(debug_assertions);
        let mut renderer = Renderer::new(window.raw_handle(), &renderer_config)?;
        #[cfg(feature = "debug-overlay")]
        renderer.set_overlay_enabled(true);
        renderer.set_culling_enabled(args.culling);

        let scene = Scene::new(&mut renderer, args.preset)?;

        let controller = FlyCamera::START;
        let mut camera = Camera::new(Projection::perspective(aspect_of(width, height)))?;
        controller.apply(&mut camera, aspect_of(width, height))?;

        // A sun low enough that vertical and horizontal faces get clearly different amounts
        // of light, which is what makes an object's orientation readable, plus enough
        // ambient that an unlit face is dark rather than black.
        let lighting = Lighting {
            direction: engine_core::math::Vec3::new(-0.45, -0.85, -0.35),
            color: engine_core::math::Vec3::new(1.0, 0.97, 0.90),
            intensity: 1.15,
            ambient_color: engine_core::math::Vec3::new(0.55, 0.62, 0.80),
            ambient_intensity: 0.28,
        };

        let report_interval = (args.report_interval_secs > 0.0)
            .then(|| Duration::from_secs_f64(args.report_interval_secs));

        log_controls();

        Ok(Self {
            window,
            renderer,
            camera,
            controller,
            scene,
            lighting,
            mouse_captured: false,
            graph: FrameGraph::new(),
            overlay_lines: Vec::with_capacity(20),
            last_overlay_update: Instant::now(),
            animation_seconds: 0.0,
            report_interval,
            last_report: Instant::now(),
        })
    }

    /// Applies one key press.
    fn handle_key(&mut self, key: KeyCode, ctx: &mut EngineContext) {
        match key {
            KeyCode::F1 => self.toggle_frozen_cull_frustum(),
            KeyCode::F2 => {
                // The stress preset plus a camera inside the field: the point of the mode is
                // that most objects are outside the frustum, which cannot happen while the
                // camera looks at the whole distribution from outside it.
                self.set_preset(PRESETS.len() - 1, ctx);
                self.controller = FlyCamera::STRESS;
            }
            KeyCode::F3 => self.set_preset(0, ctx),
            KeyCode::F4 => self.set_preset(1, ctx),
            KeyCode::F5 => self.set_preset(2, ctx),
            KeyCode::F6 => self.set_preset(3, ctx),
            KeyCode::F7 => self.set_preset(4, ctx),
            KeyCode::F8 => {
                let enabled = !self.renderer.culling_enabled();
                self.renderer.set_culling_enabled(enabled);
                // The previous setting's frame times describe a different amount of work.
                ctx.reset_clock();
                self.graph.clear();
                self.renderer.reset_present_stats();
                log::info!("frustum culling {}", on_off(enabled));
            }
            KeyCode::F9 => {
                let enabled = !self.renderer.wireframe();
                self.renderer.set_wireframe(enabled);
                log::info!("wireframe {}", on_off(enabled));
            }
            #[cfg(feature = "debug-overlay")]
            KeyCode::F10 => {
                let enabled = !self.renderer.overlay_enabled();
                self.renderer.set_overlay_enabled(enabled);
                if enabled {
                    // Rebuild on the next frame rather than at the next scheduled update.
                    self.last_overlay_update = Instant::now() - OVERLAY_UPDATE_INTERVAL;
                }
                log::info!("overlay {}", on_off(enabled));
            }
            KeyCode::F12 => {
                self.controller = FlyCamera::START;
                log::info!("camera reset");
            }
            KeyCode::Escape => self.set_mouse_captured(!self.mouse_captured),
            other => log::trace!("key pressed: {other:?}"),
        }
    }

    /// Freezes the cull frustum where the camera is, or releases it.
    ///
    /// This is the culling visualization: with the frustum frozen and the camera free, the
    /// objects culling rejected become a visible hole in the scene, which is the only way to
    /// see what a counter otherwise only asserts.
    fn toggle_frozen_cull_frustum(&mut self) {
        if self.renderer.cull_frustum_is_frozen() {
            self.renderer.set_cull_view_projection(None);
            log::info!("cull frustum released: culling follows the camera again");
        } else {
            self.renderer
                .set_cull_view_projection(Some(self.camera.view_projection()));
            log::info!(
                "cull frustum frozen at the current view: fly away to see what culling removes"
            );
        }
    }

    /// Loads preset `index` and resets the statistics windows.
    ///
    /// The old preset's frame times describe a different workload, so keeping them would
    /// blend two scenes into one set of percentiles.
    fn set_preset(&mut self, index: usize, ctx: &mut EngineContext) {
        if index == self.scene.preset() {
            return;
        }
        self.scene.load(index);
        ctx.reset_clock();
        self.graph.clear();
        self.renderer.reset_present_stats();
    }

    /// Captures or releases the mouse for look control.
    ///
    /// While captured the cursor is hidden and re-centred every frame, so it can never reach
    /// a screen edge and stop producing movement.
    fn set_mouse_captured(&mut self, captured: bool) {
        if captured == self.mouse_captured {
            return;
        }
        self.mouse_captured = captured;
        self.window.set_cursor_visible(!captured);
        if captured && let Err(error) = self.window.center_cursor() {
            log::warn!("could not centre the cursor: {error}");
        }
        log::info!(
            "mouse {}",
            if captured {
                "captured (ESC releases it)"
            } else {
                "released"
            }
        );
    }

    /// Turns this frame's cursor position into a view rotation.
    ///
    /// The offset from the client centre is used rather than the input layer's per-frame
    /// delta, because re-centring the cursor generates a movement of its own: measuring
    /// against a fixed point makes the warp invisible instead of cancelling out the player's
    /// own movement.
    fn apply_mouse_look(&mut self, ctx: &EngineContext) {
        if !self.mouse_captured {
            return;
        }
        let (center_x, center_y) = self.window.client_center();
        let (x, y) = ctx.input().mouse_position();
        let (offset_x, offset_y) = (x - center_x, y - center_y);
        if offset_x == 0 && offset_y == 0 {
            return;
        }
        self.controller.rotate(offset_x as f32, offset_y as f32);
        if let Err(error) = self.window.center_cursor() {
            log::warn!("could not centre the cursor: {error}");
        }
    }

    /// Rebuilds the overlay text and graph if the update interval has elapsed.
    #[cfg(feature = "debug-overlay")]
    fn maybe_update_overlay(&mut self, ctx: &EngineContext) {
        if !self.renderer.overlay_enabled()
            || self.last_overlay_update.elapsed() < OVERLAY_UPDATE_INTERVAL
        {
            return;
        }
        let Some(summary) = ctx.metrics().summary() else {
            return;
        };
        self.last_overlay_update = Instant::now();

        let stats = HudStats {
            summary,
            cull: self.renderer.cull_stats(),
            cull_ms: self.renderer.last_cull_ms(),
            objects: self.scene.object_count(),
            render_list: self.renderer.render_list().len(),
            scene_triangles: self.scene.scene_triangles(),
            draw_calls: self.renderer.draw_calls_last_frame(),
            triangles: self.renderer.triangles_last_frame(),
            camera_position: self.controller.position,
            resolution: self.renderer.size(),
            preset: self.scene.preset_name(),
            culling: self.renderer.culling_enabled(),
            cull_frozen: self.renderer.cull_frustum_is_frozen(),
            wireframe: self.renderer.wireframe(),
            mouse_captured: self.mouse_captured,
        };
        hud::write_lines(&mut self.overlay_lines, &stats);
        self.renderer.set_overlay_text(&self.overlay_lines);
        // Both of these append to the geometry `set_overlay_text` rebuilt, so they come
        // after it. The backdrop is sized from the text it has to cover.
        let columns = self
            .overlay_lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        self.renderer
            .push_overlay_backdrop(self.overlay_lines.len(), columns);
        let samples = self.graph.ordered();
        self.renderer
            .push_overlay_frame_graph(samples, GRAPH_BUDGET_MS);
    }

    /// Logs frame statistics if the reporting interval has elapsed.
    fn maybe_report(&mut self, ctx: &EngineContext) {
        let Some(interval) = self.report_interval else {
            return;
        };
        if self.last_report.elapsed() < interval {
            return;
        }
        let Some(summary) = ctx.metrics().summary() else {
            return;
        };
        self.last_report = Instant::now();

        let cull = self.renderer.cull_stats();
        let commands = self.renderer.command_stats();
        // The other queue. `Present` blocking and the frame fence blocking are two
        // different back-pressure points, and only one of them was ever measured.
        let sync = self.renderer.sync_stats();
        log::info!(
            "{}: {} objects | CPU mean {:.3} ms ({:.0} FPS) p50 {:.3} p95 {:.3} p99 {:.3} ms | \
             GPU {} | visible {} culled {} | cull {:.3} ms | draws {} | tris {}",
            self.scene.preset_name(),
            self.scene.object_count(),
            summary.cpu.mean_ms,
            summary.cpu.mean_fps(),
            summary.cpu.p50_ms,
            summary.cpu.p95_ms,
            summary.cpu.p99_ms,
            match summary.gpu {
                // Mean and p99 for the GPU: its timestamps lag the CPU by a few frames, so
                // the two are summarized separately and never presented as one frame.
                Some(gpu) => format!("mean {:.3} p99 {:.3} ms", gpu.mean_ms, gpu.p99_ms),
                None => "N/A".to_owned(),
            },
            cull.visible,
            cull.culled,
            self.renderer.last_cull_ms(),
            self.renderer.draw_calls_last_frame(),
            self.renderer.triangles_last_frame()
        );

        // The submission profile, on its own line. This is what Milestone 17 exists to move:
        // how many driver calls a frame makes, how many of them changed nothing, and where
        // the present stage's time actually goes.
        log::info!(
            "  submission: {} binding calls | PSO {}+{} redundant | material changes {} | \
             mesh changes {} | root CBV {}+{} redundant | VB {}+{} | IB {}+{} | \
             redundant {:.1}% | present {} | fence signal {:.3} ms | \
             fence stalls {:.1}% of frames, {:.3} ms mean",
            commands.binding_calls(),
            commands.pipeline_changes,
            commands.redundant_pipeline_binds,
            commands.material_changes,
            commands.mesh_changes,
            commands.root_cbv_binds,
            commands.redundant_root_cbv_binds,
            commands.vertex_buffer_binds,
            commands.redundant_vertex_buffer_binds,
            commands.index_buffer_binds,
            commands.redundant_index_buffer_binds,
            commands.redundant_bind_ratio() * 100.0,
            match self.renderer.present_stats() {
                Some(present) => format!(
                    "mean {:.3} p50 {:.3} p99 {:.3} max {:.3} ms",
                    present.mean_ms, present.p50_ms, present.p99_ms, present.max_ms
                ),
                None => "N/A".to_owned(),
            },
            self.renderer.last_fence_signal_ms(),
            sync.stall_ratio() * 100.0,
            sync.mean_stall_ms(),
        );
    }
}

impl Application for Showcase {
    type Error = EngineError;

    fn process_events(&mut self, ctx: &mut EngineContext) -> Result<(), EngineError> {
        self.window.pump_events();

        while let Some(event) = self.window.poll_event() {
            if let Some(input) = event.as_input() {
                ctx.input_mut().apply(input);
            }

            match event {
                WindowEvent::CloseRequested => ctx.request_exit(),
                WindowEvent::Resized { width, height } => {
                    self.renderer.resize(width, height)?;
                    self.camera.set_viewport(width, height)?;
                }
                WindowEvent::Input(InputEvent::FocusChanged { focused: false }) => {
                    // The cursor's hidden state is process-wide; leaving it hidden while
                    // another window has focus takes the pointer away from the desktop.
                    self.set_mouse_captured(false);
                }
                WindowEvent::Input(InputEvent::MouseButtonDown {
                    button: MouseButton::Left,
                }) => self.set_mouse_captured(true),
                // Physical presses only: auto-repeat must not re-trigger a toggle.
                WindowEvent::Input(InputEvent::KeyDown { key, repeat: false }) => {
                    self.handle_key(key, ctx);
                }
                _ => {}
            }
        }

        if self.window.quit_received() {
            ctx.request_exit();
        }
        Ok(())
    }

    fn update(&mut self, ctx: &mut EngineContext) -> Result<(), EngineError> {
        let delta = ctx.delta().as_secs_f32();

        self.apply_mouse_look(ctx);
        let wheel = ctx.input().wheel_delta();
        if wheel != 0.0 {
            self.controller.zoom(wheel);
        }
        self.controller.move_by_input(ctx.input(), delta);

        // A minimized window reports a zero-sized client area, which has no aspect ratio.
        let (width, height) = self.renderer.size();
        if width > 0 && height > 0 {
            self.controller
                .apply(&mut self.camera, aspect_of(width, height))?;
        }

        self.animation_seconds += delta;
        self.scene.animate(self.animation_seconds);
        Ok(())
    }

    fn render(&mut self, _ctx: &mut EngineContext) -> Result<(), EngineError> {
        self.renderer
            .render_world(&self.scene.world, &self.camera, self.lighting, CLEAR_COLOR)?;
        Ok(())
    }

    fn present(&mut self, _ctx: &mut EngineContext) -> Result<(), EngineError> {
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

    fn end_frame(&mut self, ctx: &mut EngineContext) {
        self.graph
            .record((ctx.time().real_delta().as_secs_f64() * 1e3) as f32);
        self.maybe_report(ctx);
        #[cfg(feature = "debug-overlay")]
        self.maybe_update_overlay(ctx);
    }

    fn shutdown(&mut self, ctx: &mut EngineContext) {
        self.set_mouse_captured(false);
        let frames = ctx.frame_index();
        let seconds = ctx.elapsed().as_secs_f64();
        log::info!(
            "showcase ran {frames} frames in {seconds:.3} s ({:.0} FPS average)",
            if seconds > 0.0 {
                frames as f64 / seconds
            } else {
                0.0
            }
        );
        // Recomputed rather than read from the cache, which can be a tenth of a second old.
        ctx.metrics_mut().refresh();
        if let Some(summary) = ctx.metrics().summary() {
            log::info!(
                "final CPU: mean {:.3} ms ({:.0} FPS) | p50 {:.3} | p95 {:.3} | p99 {:.3} ms | n={}",
                summary.cpu.mean_ms,
                summary.cpu.mean_fps(),
                summary.cpu.p50_ms,
                summary.cpu.p95_ms,
                summary.cpu.p99_ms,
                summary.cpu.count
            );
            if let Some(gpu) = summary.gpu {
                log::info!(
                    "final GPU: mean {:.3} ms | p50 {:.3} | p95 {:.3} | p99 {:.3} ms | n={}",
                    gpu.mean_ms,
                    gpu.p50_ms,
                    gpu.p95_ms,
                    gpu.p99_ms,
                    gpu.count
                );
            }
        }
    }
}

/// Width divided by height, guarding the zero a minimized window reports.
fn aspect_of(width: u32, height: u32) -> f32 {
    if height == 0 {
        1.0
    } else {
        width as f32 / height as f32
    }
}

/// `on` or `off`, for a log line.
fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

/// Prints the controls once at startup, so they are discoverable without the README.
fn log_controls() {
    log::info!("controls:");
    log::info!("  W A S D          move, Q/E down and up, SHIFT fast, CTRL slow");
    log::info!("  LEFT CLICK       capture the mouse for look control, ESC releases it");
    log::info!("  MOUSE WHEEL      field of view");
    log::info!("  F1               freeze/release the cull frustum (culling visualization)");
    log::info!("  F2               culling stress test: 100,000 objects");
    log::info!("  F3 F4 F5 F6 F7   10 / 100 / 1,000 / 10,000 / 100,000 objects");
    log::info!("  F8               frustum culling on/off");
    log::info!("  F9               wireframe on/off");
    log::info!("  F10              statistics overlay on/off");
    log::info!("  F12              reset the camera");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_sized_viewport_does_not_produce_an_infinite_aspect_ratio() {
        assert_eq!(aspect_of(1920, 1080), 1920.0 / 1080.0);
        assert_eq!(aspect_of(1920, 0), 1.0);
    }
}
