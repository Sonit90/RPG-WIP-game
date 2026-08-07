//! The game: window, renderer, simulation, and the per-stage work the engine loop drives.
//!
//! ```text
//! process_events   window messages -> InputState
//! update           InputState -> PlayerCommand -> Simulation (fixed 60 Hz)
//!                             -> RenderExtractor -> RenderWorld
//!                             -> ThirdPersonCamera -> Camera
//! render           RenderWorld -> cull -> RenderList -> D3D12
//! present          swap chain
//! ```
//!
//! This module is the only place in the project where all three worlds meet, and it is
//! deliberately thin: it owns handles and wiring, not rules. Gameplay rules live in
//! `engine-game`, the mapping from gameplay to rendering lives in `engine-presentation`,
//! and how a frame is recorded lives in `engine-renderer`.
//!
//! Window and device creation happen in [`App::new`] rather than in
//! [`Application::initialize`]: both produce values the application cannot exist without,
//! and holding them as `Option` to allow a two-phase init would put a check on every use
//! for no gain.

use std::time::{Duration, Instant};

use engine_core::application::{Application, EngineContext};
use engine_core::config::EngineSettings;
use engine_core::error::EngineError;
use engine_core::input::{InputEvent, KeyCode, MouseButton};
use engine_core::math::Vec3;
#[cfg(feature = "debug-overlay")]
use engine_game::ability::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::input_map::Bindings;
use engine_game::sim::Simulation;
#[cfg(feature = "debug-overlay")]
use engine_game::telemetry::{AbilityTotals, TargetTotals};
use engine_platform::{Window, WindowConfig, WindowEvent};
use engine_presentation::camera::ThirdPersonCamera;
use engine_presentation::extract::{RenderExtractor, VisualBindings};
#[cfg(feature = "debug-overlay")]
use engine_presentation::hud::{self, HudModel};
#[cfg(feature = "debug-overlay")]
use engine_presentation::nameplate::{NameplateConfig, NameplateSet};
use engine_renderer::mesh::MeshData;
use engine_renderer::{
    Camera, Lighting, MaterialDesc, MaterialHandle, MeshHandle, Projection, Renderer,
    RendererConfig,
};

use crate::args::Args;

/// Clear colour of the back buffer: a dark blue-grey that reads as sky.
const CLEAR_COLOR: [f32; 4] = [0.07, 0.09, 0.13, 1.0];

/// Half-width of the visible floor, in metres.
///
/// Larger than the arena the character is confined to, so the world does not end at the
/// wall the player runs into.
const GROUND_HALF_EXTENT: f32 = 120.0;

/// Function key that toggles the developer overlay.
#[cfg(feature = "debug-overlay")]
const OVERLAY_TOGGLE_KEY: KeyCode = KeyCode::F5;

/// Colour of a nameplate's panel: dark enough that white text reads over the arena.
///
/// Opaque, because the overlay pipeline has blending switched off. A translucent plate
/// would be nicer and is not worth a second blend state for a prototype.
#[cfg(feature = "debug-overlay")]
const PLATE_PANEL_COLOR: [f32; 4] = [0.06, 0.07, 0.09, 1.0];

/// Colour of a nameplate's name and health text.
#[cfg(feature = "debug-overlay")]
const PLATE_TEXT_COLOR: [f32; 4] = [0.95, 0.95, 0.85, 1.0];

/// Colour of the status line, so a debuff is distinguishable from the name at a glance.
#[cfg(feature = "debug-overlay")]
const PLATE_STATUS_COLOR: [f32; 4] = [0.95, 0.62, 0.25, 1.0];

/// Colour of the outline around the current target's plate.
#[cfg(feature = "debug-overlay")]
const PLATE_TARGET_OUTLINE_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Thickness of that outline, in pixels.
#[cfg(feature = "debug-overlay")]
const PLATE_OUTLINE_PIXELS: f32 = 2.0;

/// Depth added to a plate's own depth for its outline and panel.
///
/// Both are behind the text and in front of nothing else. The steps are far smaller than
/// the gap distance puts between two plates, so a plate's own parts never interleave with
/// another plate's.
#[cfg(feature = "debug-overlay")]
const PLATE_OUTLINE_DEPTH_STEP: f32 = 0.0004;

/// Depth added to a plate's own depth for its panel.
#[cfg(feature = "debug-overlay")]
const PLATE_PANEL_DEPTH_STEP: f32 = 0.0008;

/// How often the overlay text is rebuilt.
///
/// Formatting allocates, so it happens on an interval and never per frame. Ten a second is
/// responsive enough to watch a cooldown run down.
#[cfg(feature = "debug-overlay")]
const OVERLAY_UPDATE_INTERVAL: Duration = Duration::from_millis(100);

/// Window, renderer, simulation and the presentation state between them.
pub struct App {
    window: Window,
    renderer: Renderer,
    /// The gameplay simulation. Knows nothing about the renderer.
    simulation: Simulation,
    /// Gameplay to `RenderWorld`.
    extractor: RenderExtractor,
    /// The follow camera controller.
    controller: ThirdPersonCamera,
    /// What the renderer is given: a view and a projection.
    camera: Camera,
    lighting: Lighting,
    bindings: Bindings,
    /// Commands built from this frame's input. Reused; never freed.
    commands: Vec<PlayerCommand>,
    /// Whether the mouse is captured and drives the camera.
    mouse_captured: bool,
    /// Milliseconds the last render extraction took.
    extract_ms: f64,
    /// This frame's nameplates. Reused; never freed.
    #[cfg(feature = "debug-overlay")]
    nameplates: NameplateSet,
    /// Scratch for a plate's title and status lines, so drawing one allocates nothing.
    #[cfg(feature = "debug-overlay")]
    plate_title: String,
    #[cfg(feature = "debug-overlay")]
    plate_statuses: String,
    report_interval: Option<Duration>,
    last_report: Instant,
    #[cfg(feature = "debug-overlay")]
    overlay_lines: Vec<String>,
    #[cfg(feature = "debug-overlay")]
    overlay_abilities: Vec<(AbilityId, AbilityTotals)>,
    #[cfg(feature = "debug-overlay")]
    overlay_targets: Vec<TargetTotals>,
    #[cfg(feature = "debug-overlay")]
    last_overlay_update: Instant,
}

impl App {
    /// Creates the window, the renderer, the arena and everything that draws it.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the window, the D3D12 device or the scene's GPU resources
    /// cannot be created.
    pub fn new(settings: &EngineSettings, args: &Args) -> Result<Self, EngineError> {
        // Before any window exists: without it, a fullscreen window on a scaled display
        // covers only part of the screen, and the client size the renderer is given does
        // not match the pixels on screen.
        engine_platform::dpi::set_process_dpi_aware();

        let window_config = WindowConfig::new(&settings.title, settings.width, settings.height)
            .with_fullscreen(settings.display_mode.is_fullscreen());
        let window = Window::new(&window_config)?;

        // The window is the authority on its client size: fullscreen takes the monitor's
        // size, and the OS may clamp a requested size on a small display.
        let (width, height) = window.client_size();
        let mut renderer_config = RendererConfig::new(width, height).with_vsync(settings.vsync);
        renderer_config.enable_debug_layer = settings.renderer_validation;
        renderer_config.break_on_validation_error =
            settings.renderer_validation && cfg!(debug_assertions);
        let mut renderer = Renderer::new(window.raw_handle(), &renderer_config)?;
        #[cfg(feature = "debug-overlay")]
        renderer.set_overlay_enabled(settings.debug_overlay);

        let visuals = upload_visuals(&mut renderer)?;

        let arena_config = ArenaConfig::with_group(args.dummies);
        let simulation = arena::training_arena(&arena_config);
        let mut extractor = RenderExtractor::new(visuals, GROUND_HALF_EXTENT * 2.0);
        extractor.extract(&simulation);

        let controller = ThirdPersonCamera::following(simulation.player_position());
        let mut camera = Camera::new(Projection::perspective(aspect_of(width, height)))?;
        controller.apply(&mut camera, aspect_of(width, height))?;

        // A sun low enough that vertical and horizontal faces get clearly different amounts
        // of light, plus enough ambient that an unlit face is dark rather than black.
        let lighting = Lighting {
            direction: Vec3::new(-0.45, -0.85, -0.35),
            color: Vec3::new(1.0, 0.97, 0.90),
            intensity: 1.15,
            ambient_color: Vec3::new(0.55, 0.62, 0.80),
            ambient_intensity: 0.28,
        };

        let report_interval = settings
            .reports_enabled()
            .then(|| Duration::from_secs_f64(settings.report_interval_seconds));

        log_controls(args.dummies);

        Ok(Self {
            window,
            renderer,
            simulation,
            extractor,
            controller,
            camera,
            lighting,
            bindings: Bindings::default(),
            commands: Vec::with_capacity(16),
            mouse_captured: false,
            extract_ms: 0.0,
            #[cfg(feature = "debug-overlay")]
            nameplates: NameplateSet::with_capacity(32),
            #[cfg(feature = "debug-overlay")]
            plate_title: String::with_capacity(32),
            #[cfg(feature = "debug-overlay")]
            plate_statuses: String::with_capacity(64),
            report_interval,
            last_report: Instant::now(),
            #[cfg(feature = "debug-overlay")]
            overlay_lines: Vec::with_capacity(32),
            #[cfg(feature = "debug-overlay")]
            overlay_abilities: Vec::with_capacity(engine_game::ABILITY_SLOTS),
            #[cfg(feature = "debug-overlay")]
            overlay_targets: Vec::with_capacity(32),
            #[cfg(feature = "debug-overlay")]
            last_overlay_update: Instant::now(),
        })
    }

    /// Applies one physical key press that is not a gameplay binding.
    ///
    /// Gameplay keys are *not* handled here: they are read from `InputState` by
    /// [`Bindings::write_commands`], so the simulation sees commands rather than events and
    /// a replay can supply the same commands without a keyboard.
    fn handle_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Escape => self.set_mouse_captured(false),
            #[cfg(feature = "debug-overlay")]
            OVERLAY_TOGGLE_KEY => {
                let enabled = !self.renderer.overlay_enabled();
                self.renderer.set_overlay_enabled(enabled);
                if enabled {
                    // Rebuild on the next frame rather than at the next scheduled update.
                    self.last_overlay_update = Instant::now() - OVERLAY_UPDATE_INTERVAL;
                }
                log::info!("overlay {}", if enabled { "shown" } else { "hidden" });
            }
            KeyCode::F9 => {
                let enabled = !self.renderer.wireframe();
                self.renderer.set_wireframe(enabled);
                log::info!("wireframe {}", if enabled { "on" } else { "off" });
            }
            other => log::trace!("key pressed: {other:?}"),
        }
    }

    /// Captures or releases the mouse for camera control.
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

    /// Turns this frame's cursor position into a camera rotation.
    ///
    /// The offset from the client centre is used rather than the input layer's per-frame
    /// delta, because re-centring the cursor generates a movement of its own: measuring
    /// against a fixed point makes the warp invisible instead of cancelling out the
    /// player's own movement.
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

    /// Rebuilds and draws the nameplates for every nearby enemy.
    ///
    /// Every frame, not on the overlay's update interval: a plate is anchored to a
    /// projected world position, so it is wrong the moment the camera turns.
    ///
    /// Drawn independently of the developer overlay's F5 toggle — a nameplate is game UI,
    /// and F5 hides the statistics readout. It does depend on the `debug-overlay` *feature*,
    /// because the overlay is the only text renderer the engine has; see the known
    /// limitations in `docs/phase3.md`.
    #[cfg(feature = "debug-overlay")]
    fn draw_nameplates(&mut self) {
        let metrics = self.renderer.overlay_metrics();
        let config = NameplateConfig {
            cell_pixels: metrics.cell_pixels,
            glyph_advance_pixels: metrics.glyph_advance_pixels,
            line_advance_pixels: metrics.line_advance_pixels,
            glyph_height_pixels: metrics.glyph_height_pixels,
            ..NameplateConfig::default()
        };

        let view_projection = self.camera.view_projection();
        let viewport = self.renderer.size();
        self.nameplates
            .build(&self.simulation, &view_projection, viewport, &config);

        self.renderer.begin_dynamic_overlay();
        for index in 0..self.nameplates.plates().len() {
            let plate = self.nameplates.plates()[index];
            let rect = (plate.origin.0, plate.origin.1, plate.size.0, plate.size.1);

            self.renderer.push_overlay_rect(
                rect,
                PLATE_PANEL_COLOR,
                plate.depth + PLATE_PANEL_DEPTH_STEP,
            );
            // The outline is what says "this is what your abilities will hit", so it is the
            // target's and nothing else's.
            if plate.is_target {
                self.renderer.push_overlay_outline(
                    rect,
                    PLATE_OUTLINE_PIXELS,
                    PLATE_TARGET_OUTLINE_COLOR,
                    plate.depth + PLATE_OUTLINE_DEPTH_STEP,
                );
            }

            plate.write_title(&mut self.plate_title);
            let text_x = plate.origin.0 + config.padding_pixels;
            let text_y = plate.origin.1 + config.padding_pixels;
            self.renderer.push_overlay_text_at(
                (text_x, text_y),
                &self.plate_title,
                PLATE_TEXT_COLOR,
                plate.depth,
            );

            if plate.write_statuses(&mut self.plate_statuses) {
                self.renderer.push_overlay_text_at(
                    (text_x, text_y + config.line_advance_pixels),
                    &self.plate_statuses,
                    PLATE_STATUS_COLOR,
                    plate.depth,
                );
            }
        }
    }

    /// Logs a combat and performance line if the reporting interval has elapsed.
    ///
    /// Logging allocates while formatting, so it happens on an interval and never per
    /// frame.
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

        // The last advance that ticked, not the last advance: most frames at these rates
        // run no ticks, and their zero would read as "the simulation is free".
        let stats = self.simulation.last_tick_stats();
        let meter = self.simulation.meter();
        let cull = self.renderer.cull_stats();
        log::info!(
            "CPU mean {:.3} ms ({:.0} FPS) p99 {:.3} | GPU {} | sim {:.3} ms ({} ticks) | \
             extract {:.3} ms | cull {:.3} ms | visible {} | draws {}",
            summary.cpu.mean_ms,
            summary.cpu.mean_fps(),
            summary.cpu.p99_ms,
            match summary.gpu {
                Some(gpu) => format!("mean {:.3} p99 {:.3} ms", gpu.mean_ms, gpu.p99_ms),
                None => "N/A".to_owned(),
            },
            stats.total_ms(),
            stats.ticks,
            self.extract_ms,
            self.renderer.last_cull_ms(),
            cull.visible,
            self.renderer.draw_calls_last_frame(),
        );
        log::info!(
            "  combat: {:.0} damage | {:.0} DPS over {:.1} s | {} deaths | {} events",
            meter.total_damage(),
            meter.dps(),
            meter.combat_seconds(),
            meter.deaths(),
            self.simulation.event_stream().total_recorded(),
        );
    }

    /// Rebuilds the overlay text if the update interval has elapsed.
    #[cfg(feature = "debug-overlay")]
    fn maybe_update_overlay(&mut self, ctx: &EngineContext) {
        if !self.renderer.overlay_enabled()
            || self.last_overlay_update.elapsed() < OVERLAY_UPDATE_INTERVAL
        {
            return;
        }
        self.last_overlay_update = Instant::now();

        let cull = self.renderer.cull_stats();
        let model = HudModel {
            simulation: &self.simulation,
            summary: ctx.metrics().summary(),
            sim: self.simulation.last_tick_stats(),
            ticks_this_frame: self.simulation.stats().ticks,
            extract_ms: self.extract_ms,
            cull_ms: self.renderer.last_cull_ms(),
            visible: cull.visible,
            culled: cull.culled,
            draw_calls: self.renderer.draw_calls_last_frame(),
            resolution: self.renderer.size(),
        };
        hud::write_lines(
            &mut self.overlay_lines,
            &model,
            &mut self.overlay_abilities,
            &mut self.overlay_targets,
        );
        self.renderer.set_overlay_text(&self.overlay_lines);
        // Appends to the geometry `set_overlay_text` rebuilt, so it comes after it. The
        // backdrop is sized from the text it has to cover.
        let columns = self
            .overlay_lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        self.renderer
            .push_overlay_backdrop(self.overlay_lines.len(), columns);
    }

    /// Logs a final summary of the run.
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
        log::info!(
            "simulated {} ticks ({:.2} s of gameplay), {} combat events, {:.0} damage",
            self.simulation.tick_count(),
            self.simulation.time(),
            self.simulation.event_stream().total_recorded(),
            self.simulation.meter().total_damage(),
        );

        // Recompute rather than reading the cached summary: the last refresh can be up to
        // a quarter of a second old, and the closing numbers should cover the whole window.
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
    type Error = EngineError;

    /// Drains window messages and applies them.
    fn process_events(&mut self, ctx: &mut EngineContext) -> Result<(), EngineError> {
        self.window.pump_events();

        while let Some(event) = self.window.poll_event() {
            // Every input event updates the engine's input state, whether or not this
            // application acts on it: gameplay reads state and produces commands.
            if let Some(input) = event.as_input() {
                ctx.input_mut().apply(input);
            }

            match event {
                WindowEvent::CloseRequested => {
                    log::info!("close requested");
                    ctx.request_exit();
                }
                WindowEvent::Resized { width, height } => {
                    self.renderer.resize(width, height)?;
                    if width > 0 && height > 0 {
                        self.camera.set_viewport(width, height)?;
                    }
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
                    self.handle_key(key);
                }
                _ => {}
            }
        }

        if self.window.quit_received() {
            ctx.request_exit();
        }
        Ok(())
    }

    /// Input, simulation, extraction and the camera.
    ///
    /// Each is scoped separately, so the profiler attributes a frame to a stage of the
    /// gameplay pipeline rather than to "update".
    fn update(&mut self, ctx: &mut EngineContext) -> Result<(), EngineError> {
        let profiler = ctx.profiler();

        {
            engine_core::profile_scope!(profiler, "Input");
            self.apply_mouse_look(ctx);
            let wheel = ctx.input().wheel_delta();
            if wheel != 0.0 {
                self.controller.zoom(wheel);
            }
            self.commands.clear();
            self.bindings
                .write_commands(ctx.input(), self.controller.yaw, &mut self.commands);
            self.simulation.push_commands(self.commands.iter().copied());
        }

        {
            engine_core::profile_scope!(profiler, "Simulation");
            // The *real* delta: the fixed timestep is what decouples gameplay from the frame
            // rate, and feeding it a scaled or clamped delta would do that job twice.
            self.simulation.advance(ctx.time().real_delta());
        }

        {
            engine_core::profile_scope!(profiler, "Extraction");
            let started = Instant::now();
            self.extractor.extract(&self.simulation);
            self.extract_ms = started.elapsed().as_secs_f64() * 1e3;
        }

        {
            engine_core::profile_scope!(profiler, "Camera");
            // The render delta, not the simulation step: the camera is presentation and is
            // expected to move every frame, which is what stops a 60 Hz simulation from
            // looking like 60 FPS.
            self.controller.follow(
                self.simulation.player_position(),
                ctx.time().real_delta().as_secs_f32(),
            );
            let (width, height) = self.renderer.size();
            if width > 0 && height > 0 {
                self.controller
                    .apply(&mut self.camera, aspect_of(width, height))?;
            }
        }

        // After the camera, because a plate is a projection through it: building plates
        // from last frame's camera lags them a frame behind the enemies they label.
        #[cfg(feature = "debug-overlay")]
        {
            engine_core::profile_scope!(profiler, "Nameplates");
            self.draw_nameplates();
        }

        Ok(())
    }

    fn render(&mut self, _ctx: &mut EngineContext) -> Result<(), EngineError> {
        self.renderer.render_world(
            self.extractor.render_world(),
            &self.camera,
            self.lighting,
            CLEAR_COLOR,
        )?;
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

    /// Reporting and overlay text only; the engine has already recorded the frame.
    fn end_frame(&mut self, ctx: &mut EngineContext) {
        self.maybe_report(ctx);
        #[cfg(feature = "debug-overlay")]
        self.maybe_update_overlay(ctx);
    }

    fn shutdown(&mut self, ctx: &mut EngineContext) {
        self.set_mouse_captured(false);
        self.report_final(ctx);
    }
}

/// Uploads the arena's meshes and materials and describes what each thing is drawn with.
///
/// Boxes and a plane: the prototype is about combat mechanics, and modelled characters are
/// Phase 5's business. Every shape goes through the same `create_mesh` path the showcase
/// uses, so what is on screen is evidence about the renderer.
fn upload_visuals(renderer: &mut Renderer) -> Result<VisualBindings, EngineError> {
    let unit_cube = MeshData::cube(1.0);
    let ground = MeshData::plane(1.0);
    let cube_bounds = unit_cube.bounds();
    let ground_bounds = ground.bounds();

    let cube_mesh: MeshHandle = renderer.create_mesh(&unit_cube)?;
    let ground_mesh: MeshHandle = renderer.create_mesh(&ground)?;

    let player_material: MaterialHandle =
        renderer.create_material(MaterialDesc::colored(0.30, 0.62, 0.90).with_roughness(0.45))?;
    let dummy_material =
        renderer.create_material(MaterialDesc::colored(0.86, 0.34, 0.26).with_roughness(0.60))?;
    let dead_material =
        renderer.create_material(MaterialDesc::colored(0.28, 0.24, 0.22).with_roughness(0.90))?;
    let ground_material =
        renderer.create_material(MaterialDesc::colored(0.16, 0.20, 0.17).with_roughness(0.95))?;
    let marker_material = renderer.create_material(
        MaterialDesc::colored(0.98, 0.86, 0.25)
            .with_roughness(0.30)
            .with_emissive(Vec3::new(0.5, 0.42, 0.05)),
    )?;

    Ok(VisualBindings {
        player_mesh: cube_mesh,
        player_bounds: cube_bounds,
        player_material,
        dummy_mesh: cube_mesh,
        dummy_bounds: cube_bounds,
        dummy_material,
        dead_material,
        ground_mesh,
        ground_bounds,
        ground_material,
        marker_mesh: cube_mesh,
        marker_bounds: cube_bounds,
        marker_material,
    })
}

/// Width divided by height, guarding the zero a minimized window reports.
fn aspect_of(width: u32, height: u32) -> f32 {
    if height == 0 {
        1.0
    } else {
        width as f32 / height as f32
    }
}

/// Prints the controls once at startup, so they are discoverable without the README.
fn log_controls(dummies: usize) {
    log::info!("training arena: 1 dummy plus a group of {dummies}");
    log::info!("controls:");
    log::info!("  W A S D          move, relative to the camera");
    log::info!("  SPACE            jump");
    log::info!("  1 2 3            fireball, ignite, detonate (4-0 are placeholders)");
    log::info!("  TAB              cycle target");
    log::info!("  R                reset the dummies and the combat meter");
    log::info!("  LEFT CLICK       capture the mouse for camera control, ESC releases it");
    log::info!("  MOUSE WHEEL      camera distance");
    log::info!("  F5               developer overlay on/off");
    log::info!("  F9               wireframe on/off");
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_sized_viewport_does_not_produce_an_infinite_aspect_ratio() {
        assert_eq!(aspect_of(1920, 1080), 1920.0 / 1080.0);
        assert_eq!(aspect_of(1920, 0), 1.0);
    }
}
