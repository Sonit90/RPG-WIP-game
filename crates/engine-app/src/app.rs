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
use engine_core::input::{InputEvent, KeyCode};
use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::demo::{self, DemoScene};
use engine_game::firemage::FireAbility;
use engine_game::input_map::Bindings;
use engine_game::sim::Simulation;
use engine_platform::{Window, WindowConfig, WindowEvent};
use engine_presentation::camera::ThirdPersonCamera;
#[cfg(feature = "debug-overlay")]
use engine_presentation::collision_debug::{CollisionDebug, DebugBindings};
#[cfg(feature = "debug-overlay")]
use engine_presentation::collision_hud;
use engine_presentation::damage_numbers::FloatingNumbers;
use engine_presentation::extract::{RenderExtractor, VisualBindings};
use engine_presentation::game_ui::GameUi;
#[cfg(feature = "debug-overlay")]
use engine_presentation::hud::{self, HudModel};
use engine_presentation::mouse_look::{LookChange, MouseLook, RelativeMotion};
use engine_presentation::nameplate::{NameplateConfig, NameplateSet};
use engine_presentation::snapshot::UiSnapshot;
use engine_presentation::ui::talent_menu::{TalentMenu, TalentMenuOutcome, TalentTheme};
use engine_presentation::ui::targeting::{GroundTargeting, TargetingOutcome};
use engine_presentation::ui::{MenuAction, MenuScratch, MenuSystem, MenuTheme, UiInput};
use engine_render_data::ui::UiRect;
use engine_renderer::mesh::MeshData;
use engine_renderer::{
    Camera, Lighting, MaterialDesc, MaterialHandle, MeshHandle, Projection, Renderer,
    RendererConfig, UiDrawList,
};
use engine_settings::SettingsManager;

use crate::args::{Args, Scene};

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

/// Function key that toggles the collision debug drawing.
///
/// F3 by the project's existing convention of putting developer toggles on the function
/// row beside F5 (overlay) and F9 (wireframe).
#[cfg(feature = "debug-overlay")]
const COLLISION_DEBUG_KEY: KeyCode = KeyCode::F3;

/// How far above the floor the targeting disc sits, in metres.
///
/// Coplanar with the floor would be a depth-buffer tie and would flicker; a few centimetres
/// reads as painted on the ground rather than hovering over it.
const DECAL_HEIGHT: f32 = 0.03;

/// How far above the fill the indicator's ring sits, in metres.
const DECAL_RING_LIFT: f32 = 0.02;

/// How fast the indicator's ring breathes, in radians per second.
const DECAL_PULSE_RATE: f32 = 3.0;

/// How much the ring grows and shrinks as it breathes, as a fraction of its radius.
const DECAL_PULSE_DEPTH: f32 = 0.02;

/// Size of the ring drawn over an enemy inside the targeting circle, in pixels.
const AFFECTED_MARK_SIZE: f32 = 46.0;

/// The key that casts the Ultimate. It is not on the bar, so it has a key of its own.
const ULTIMATE_KEY: KeyCode = KeyCode::R;

/// Glyph scale the game UI draws text at.
///
/// The font is a 5x7 bitmap; 2.0 gives a 10x14 pixel cell, which is what the Phase 3
/// overlay used and is legible at 1080p without being a magnified blur.
const UI_TEXT_SCALE: f32 = 2.0;

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
    /// Which scenario is loaded. Chosen once, at startup, by `--scene`.
    scene: Scene,
    /// The collision demo's stations and labels. `None` in the combat scene.
    demo: Option<DemoScene>,
    /// The collision demo's world-space labels. Its own draw list, because the demo scene
    /// draws none of the combat interface.
    collision_ui: UiDrawList,
    /// The collision world, drawn. Developer-only, and off in the combat scene.
    #[cfg(feature = "debug-overlay")]
    collision_debug: CollisionDebug,
    /// Commands built from this frame's input. Reused; never freed.
    commands: Vec<PlayerCommand>,
    /// The pause and settings menus. Owns no gameplay and no file: it emits actions.
    menu: MenuSystem,
    /// This frame's menu actions, copied out of the menu so they can be applied to `self`.
    /// Reused; never freed.
    menu_actions: Vec<MenuAction>,
    /// How the menus are coloured and sized.
    menu_theme: MenuTheme,
    /// Scratch the menu formats its numbers into. Reused; never freed.
    menu_scratch: MenuScratch,
    /// The player's settings and the file they live in.
    settings: SettingsManager,
    /// The one ground-targeting circle, shared by Meteor, Firestorm and Cataclysm.
    targeting: GroundTargeting,
    /// The talent menu, opened from the button in the top-right corner.
    talent_menu: TalentMenu,
    /// How the talent menu is coloured.
    talent_theme: TalentTheme,
    /// Scratch the talent menu formats its numbers into. Reused; never freed.
    talent_scratch: String,
    /// Which ability button the pointer is over, for the tooltip.
    hovered_ability: Option<FireAbility>,
    /// The area indicator drawn on the ground while an ability is being placed.
    ///
    /// Two render entities rather than a screen-space circle: an area on the floor has to
    /// lie *on the floor*, or the player cannot tell what it will cover. The fill is the
    /// shaded footprint and the ring is its bright edge, which is what makes the shape
    /// readable over grass, stone or a corpse.
    targeting_decal: Option<(
        engine_render_data::RenderEntity,
        engine_render_data::RenderEntity,
    )>,
    /// The meshes and materials the indicator is drawn with.
    decal_visuals: DecalVisuals,
    /// Enemies inside the circle this frame. Reused; never freed.
    affected: Vec<engine_game::EntityId>,
    /// Whether the right button is held and the mouse is turning the camera.
    mouse_look: MouseLook,
    /// Mouse movement to rotate by, with the engine's own re-centring taken back out.
    motion: RelativeMotion,
    /// Where the cursor was when mouse-look began, in client pixels.
    ///
    /// Put back when the button is released, so turning the camera does not walk the
    /// pointer to the middle of the screen.
    cursor_before_look: (i32, i32),
    /// Milliseconds the last render extraction took.
    extract_ms: f64,
    /// Milliseconds the last UI *layout* took: nameplates, snapshot and draw-list build.
    ///
    /// The renderer measures the batching that follows it separately, so the two together
    /// are the whole CPU cost of the interface.
    ui_ms: f64,
    /// This frame's nameplates. Reused; never freed.
    nameplates: NameplateSet,
    /// What the UI is allowed to know about the simulation, rebuilt each frame.
    snapshot: UiSnapshot,
    /// The game UI's layout. Owns its draw list across frames.
    game_ui: GameUi,
    /// Floating damage numbers in flight. Fixed capacity; never freed.
    numbers: FloatingNumbers,
    report_interval: Option<Duration>,
    last_report: Instant,
    #[cfg(feature = "debug-overlay")]
    overlay_lines: Vec<String>,
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

        renderer.set_ui_enabled(args.ui);

        let visuals = upload_visuals(&mut renderer)?;
        let decal_visuals = upload_decal_visuals(&mut renderer)?;
        #[cfg(feature = "debug-overlay")]
        let debug_visuals = upload_debug_visuals(&mut renderer)?;

        let (simulation, demo) = match args.scene {
            Scene::Combat => (
                arena::training_arena(&ArenaConfig::with_group(args.dummies)),
                None,
            ),
            Scene::Collision => {
                let (simulation, scene) = demo::collision_demo();
                (simulation, Some(scene))
            }
        };
        let mut extractor = RenderExtractor::new(visuals, GROUND_HALF_EXTENT * 2.0);
        if args.scene.is_collision() {
            // The demo course brings its own floor: a slab whose top face is at y = 0. The
            // extractor's floor plane is at y = 0 too, and two coplanar surfaces are a
            // depth-buffer tie — which reads as the whole floor flickering. Dropped to the
            // course's catch floor, where it is the thing a long fall lands on instead.
            extractor.set_ground_height(demo::VOID_Y);
        }
        extractor.extract(&simulation);

        // The collision course is only visible through the debug view, so in that scene the
        // view starts on: it *is* the scene's picture, not an overlay on top of one. F3
        // still turns it off, which is how a developer checks that turning it off works.
        #[cfg(feature = "debug-overlay")]
        let mut collision_debug = CollisionDebug::new(debug_visuals);
        #[cfg(feature = "debug-overlay")]
        if args.scene.is_collision() {
            collision_debug.set_enabled(extractor.render_world_mut(), true);
            collision_debug.rebuild(extractor.render_world_mut(), &simulation);
        }

        // Settings first: the camera is built with the sensitivity the player last chose,
        // so the first mouse movement of the session already uses it.
        // `settings` here is the engine's; this is the player's, which is a different thing
        // with a different lifetime: it changes while the game runs and it is written back.
        let game_settings = SettingsManager::load_from_game_directory();
        log::info!(
            "settings: {} (camera sensitivity {:.2})",
            game_settings.path().display(),
            game_settings.settings().camera_sensitivity
        );

        let mut controller = ThirdPersonCamera::following(simulation.player_position());
        controller.set_sensitivity(game_settings.settings().camera_sensitivity);
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

        // Built before the first frame so the UI never sees a default-constructed snapshot.
        let snapshot = UiSnapshot::build(&simulation);

        let report_interval = settings
            .reports_enabled()
            .then(|| Duration::from_secs_f64(settings.report_interval_seconds));

        log_controls(args, demo.as_ref());

        Ok(Self {
            window,
            renderer,
            simulation,
            extractor,
            controller,
            camera,
            lighting,
            bindings: Bindings::default(),
            scene: args.scene,
            demo,
            collision_ui: UiDrawList::with_capacity(64, 1_024),
            #[cfg(feature = "debug-overlay")]
            collision_debug,
            commands: Vec::with_capacity(16),
            menu: MenuSystem::new(game_settings.settings(), env!("CARGO_PKG_VERSION")),
            menu_actions: Vec::with_capacity(4),
            menu_theme: MenuTheme::default(),
            menu_scratch: MenuScratch::new(),
            settings: game_settings,
            targeting: GroundTargeting::new(),
            talent_menu: TalentMenu::new(),
            talent_theme: TalentTheme::default(),
            talent_scratch: String::with_capacity(32),
            hovered_ability: None,
            targeting_decal: None,
            decal_visuals,
            affected: Vec::with_capacity(32),
            mouse_look: MouseLook::new(),
            motion: RelativeMotion::new(),
            cursor_before_look: (0, 0),
            extract_ms: 0.0,
            ui_ms: 0.0,
            nameplates: NameplateSet::with_capacity(32),
            snapshot,
            game_ui: GameUi::new(),
            numbers: FloatingNumbers::new(),
            report_interval,
            last_report: Instant::now(),
            #[cfg(feature = "debug-overlay")]
            overlay_lines: Vec::with_capacity(32),
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
            #[cfg(feature = "debug-overlay")]
            COLLISION_DEBUG_KEY => {
                let enabled = !self.collision_debug.enabled();
                self.collision_debug
                    .set_enabled(self.extractor.render_world_mut(), enabled);
                log::info!("collision debug {}", if enabled { "on" } else { "off" });
            }
            other => {
                if !self.handle_station_key(other) {
                    log::trace!("key pressed: {other:?}");
                }
            }
        }
    }

    /// Puts the character at a numbered test station, or back at the start.
    ///
    /// Collision scene only. In the combat scene the digits are the ability bar and `R`
    /// resets the dummies, and both are handled by [`Bindings`] rather than here.
    ///
    /// Returns whether the key meant anything.
    fn handle_station_key(&mut self, key: KeyCode) -> bool {
        let Some(scene) = self.demo.as_ref() else {
            return false;
        };
        let station = match key {
            KeyCode::R => scene.stations.first().copied(),
            // The digit that was pressed names the station directly: 3 is the steps. Only
            // 1 to 9 are searched, because a station key is a digit and 0 is not one of
            // them in this scene.
            other => (1..=9)
                .find(|number| KeyCode::digit_key(*number) == Some(other))
                .and_then(|number| scene.station(number)),
        };
        let Some(station) = station else {
            return false;
        };

        self.simulation.push_command(PlayerCommand::Teleport {
            position: station.spawn,
        });
        log::info!(
            "station {} {}: {}",
            station.key,
            station.name,
            station.instruction
        );
        true
    }

    /// Hides the cursor and parks it at the centre while the camera is being turned.
    ///
    /// The position it was at is remembered, not the centre: the cursor goes back where the
    /// player pressed the button, so holding the right button to look around leaves the
    /// pointer where they left it.
    fn begin_mouse_look(&mut self, cursor: (i32, i32)) {
        self.cursor_before_look = cursor;
        // Movement is measured from here on. Where the cursor happens to be is the origin,
        // never a rotation.
        self.motion.start_at(cursor);
        self.window.set_cursor_visible(false);
        self.recenter_cursor();
    }

    /// Warps the cursor to the client centre and records that the warp is not movement.
    ///
    /// Skipped while a previous warp has not been seen yet: warping twice would leave the
    /// first one unmatched, and it would be counted as a rotation when its message arrived.
    fn recenter_cursor(&mut self) {
        if self.motion.warp_pending() || self.motion.cursor() == self.window.client_center() {
            return;
        }
        match self.window.center_cursor() {
            Ok(()) => self.motion.note_warp(self.window.client_center()),
            Err(error) => log::warn!("could not centre the cursor: {error}"),
        }
    }

    /// Shows the cursor again and puts it back where mouse-look started.
    fn end_mouse_look(&mut self) {
        self.window.set_cursor_visible(true);
        let (x, y) = self.cursor_before_look;
        if let Err(error) = self.window.set_cursor_position(x, y) {
            log::warn!("could not restore the cursor: {error}");
        }
    }

    /// Ends mouse-look if it is running. For focus loss and shutdown.
    fn cancel_mouse_look(&mut self) {
        if self.mouse_look.cancel().is_some() {
            self.end_mouse_look();
        }
    }

    /// Runs the class's interface for this frame: buttons, tooltips, targeting, talents.
    ///
    /// The order matters. The talent menu is on top, so it eats the click first; the
    /// targeting circle is next, because a click while placing is a placement and not a
    /// button press; the bar is last. Every outcome leaves as a command, so what the mouse
    /// did is in the same ordered stream as what the keyboard did.
    fn update_firemage_ui(&mut self, ctx: &mut EngineContext) {
        let viewport = self.renderer.size();
        let ui_input = UiInput::from_input(ctx.input());
        self.snapshot.refresh(&self.simulation);

        // The talent menu, which covers everything while it is up.
        if self.talent_menu.is_open() {
            match self.talent_menu.update(&ui_input, &self.snapshot, viewport) {
                TalentMenuOutcome::Purchase(talent) => {
                    self.simulation
                        .push_command(PlayerCommand::PurchaseTalent(talent));
                }
                TalentMenuOutcome::Idle | TalentMenuOutcome::Closed => {}
            }
            return;
        }

        // The talent button, in the top-right corner.
        let talent_button = GameUi::talent_button_rect(viewport);
        if ui_input.pointer_pressed
            && talent_button.contains(ui_input.pointer.0, ui_input.pointer.1)
        {
            self.talent_menu.open();
            return;
        }

        // Placing a ground-targeted ability.
        if self.targeting.is_active() {
            let caster = self.simulation.player_position();
            let view_projection = self.camera.view_projection();
            match self
                .targeting
                .update(&ui_input, &view_projection, viewport, caster)
            {
                TargetingOutcome::Confirmed { ability, position } => {
                    let command = match ability.bar_slot() {
                        Some(slot) => PlayerCommand::UseAbilityAt { slot, position },
                        None => PlayerCommand::UseUltimate { position },
                    };
                    self.simulation.push_command(command);
                }
                TargetingOutcome::Cancelled
                | TargetingOutcome::Choosing
                | TargetingOutcome::Idle => {}
            }
            return;
        }

        // The bar: what the pointer is over, and what it clicked.
        self.hovered_ability = None;
        let mut pressed = None;
        for (slot, view) in self.snapshot.abilities.iter().enumerate() {
            let rect = GameUi::slot_rect(slot, viewport);
            if rect.contains(ui_input.pointer.0, ui_input.pointer.1) {
                self.hovered_ability = Some(view.ability);
                if ui_input.pointer_pressed {
                    pressed = Some(view.ability);
                }
            }
        }
        let ultimate = GameUi::ultimate_rect(viewport);
        if ultimate.contains(ui_input.pointer.0, ui_input.pointer.1) {
            self.hovered_ability = Some(self.snapshot.ultimate.ability);
            if ui_input.pointer_pressed {
                pressed = Some(self.snapshot.ultimate.ability);
            }
        }

        // A key press starts a ground-targeted ability too: the key opens the circle rather
        // than casting, which is why the input map does not send a command for those.
        for (slot, view) in self.snapshot.abilities.iter().enumerate() {
            if !view.ground_targeted {
                continue;
            }
            let Some(key) = engine_game::input_map::Bindings::ability_key(slot) else {
                continue;
            };
            if ctx.input().was_key_pressed(key) {
                pressed = Some(view.ability);
            }
        }
        if ctx.input().was_key_pressed(ULTIMATE_KEY) {
            pressed = Some(FireAbility::Cataclysm);
        }

        if let Some(ability) = pressed {
            self.cast_or_target(ability);
        }
    }

    /// Casts an ability, or opens the targeting circle if it is placed with the mouse.
    fn cast_or_target(&mut self, ability: FireAbility) {
        if ability.definition().target.is_ground_targeted() {
            // Mouse-look would fight the circle for the cursor, so it ends here.
            self.cancel_mouse_look();
            self.targeting.begin(ability);
            return;
        }
        match ability.bar_slot() {
            Some(slot) => self
                .simulation
                .push_command(PlayerCommand::UseAbility(slot)),
            None => {
                // The Ultimate is not on the bar and has no slot; it is placed, so this is
                // only reached if a future Ultimate stops being ground-targeted.
                self.targeting.begin(ability);
            }
        }
    }

    /// Runs the menus for this frame and carries out what they asked for.
    ///
    /// The menu never performs an action itself: it returns [`MenuAction`]s and this is the
    /// only place that turns one into a quit, a saved file or a camera change. That is what
    /// keeps a check for which screen is open out of the renderer and the simulation.
    fn update_menu(&mut self, ctx: &mut EngineContext) {
        self.menu.layout(self.renderer.size());

        let ui_input = UiInput::from_input(ctx.input());

        // Escape has a chain of meanings, most local first: close the talent window, then
        // cancel a placement, then stop a cast, and only then open the pause menu. Each
        // press backs out of exactly one thing, which is what makes pressing it twice from
        // the talent window land on the menu.
        if ui_input.cancel {
            if self.talent_menu.is_open() {
                self.talent_menu.close();
                return;
            }
            if self.targeting.is_active() {
                self.targeting.cancel();
                return;
            }
            if self.simulation.cancel_cast() {
                log::debug!("cast cancelled");
                return;
            }
        }
        // Copied out: applying an action needs `&mut self`, and the slice borrows the menu.
        self.menu_actions.clear();
        self.menu_actions
            .extend_from_slice(self.menu.update(&ui_input));

        for index in 0..self.menu_actions.len() {
            match self.menu_actions[index] {
                MenuAction::Opened => {
                    // The cursor has to come back so the player can click a button with it.
                    // The game itself keeps running: this is a multiplayer world, and it
                    // does not stop because one player opened a menu.
                    self.cancel_mouse_look();
                    self.menu.apply_settings(self.settings.settings());
                    log::info!("menu opened");
                }
                MenuAction::Closed => log::info!("menu closed"),
                MenuAction::OpenedSettings => {
                    // The screen shows what is in the settings, not what it last showed.
                    self.menu.apply_settings(self.settings.settings());
                }
                MenuAction::ClosedSettings => {}
                MenuAction::QuitRequested => {
                    log::info!("quit requested from the menu");
                    // Through the engine's own lifecycle: the loop finishes this frame,
                    // `shutdown` runs, the renderer drains its frames in flight and the
                    // window closes. Nothing here terminates the process itself.
                    ctx.request_exit();
                }
                MenuAction::SensitivityChanged(value) => self.apply_sensitivity(value, false),
                MenuAction::SensitivityCommitted(value) => self.apply_sensitivity(value, true),
            }
        }
    }

    /// Applies a camera sensitivity, and writes it to disk if the interaction has finished.
    ///
    /// The value goes to the settings first and reaches the camera from there, so what the
    /// camera turns at is always what would be written to the file, snapping included.
    fn apply_sensitivity(&mut self, value: f32, persist: bool) {
        self.settings.set_camera_sensitivity(value);
        self.controller
            .set_sensitivity(self.settings.settings().camera_sensitivity);
        if persist && self.settings.save_if_dirty() {
            log::info!(
                "settings: camera sensitivity {:.2} saved",
                self.settings.settings().camera_sensitivity
            );
        }
    }

    /// Turns this frame's mouse *movement* into a camera rotation, while the look button is
    /// held.
    ///
    /// Movement only: the cursor's position never enters the rotation, so where the player
    /// happened to be pointing when they pressed the button does not turn the camera. The
    /// engine's own re-centring — which keeps the cursor away from the screen edge, where
    /// it would stop producing movement — is cancelled by [`RelativeMotion`] rather than
    /// measured around, so a warp whose message arrives a frame late costs nothing either.
    ///
    /// The button's *state* drives this, not its press and release events, so the camera
    /// cannot be left rotating after a release: [`MouseLook`] reads what is held now.
    fn apply_mouse_look(&mut self, ctx: &EngineContext) {
        match self.mouse_look.update(ctx.input()) {
            Some(LookChange::Began) => {
                self.begin_mouse_look(ctx.input().mouse_position());
                return;
            }
            Some(LookChange::Ended) => {
                self.end_mouse_look();
                return;
            }
            None => {}
        }
        if !self.mouse_look.is_active() {
            return;
        }
        let (delta_x, delta_y) = self.motion.take();
        if delta_x != 0 || delta_y != 0 {
            self.controller.rotate(delta_x as f32, delta_y as f32);
        }
        self.recenter_cursor();
    }

    /// Rebuilds the whole game UI and hands it to the renderer.
    ///
    /// Every frame, not on the overlay's update interval: a nameplate is anchored to a
    /// projected world position, so it is wrong the moment the camera turns.
    ///
    /// **Not** behind the `debug-overlay` feature. This is what the player sees, and it goes
    /// through the batched UI pass rather than the developer overlay's per-glyph geometry —
    /// which is what makes it affordable. The Phase 3 limitation that a
    /// `--no-default-features` build had no nameplates is gone.
    fn build_ui(&mut self) {
        if !self.renderer.ui_enabled() {
            // Skip the layout too, not just the draw. Measuring "the UI off" has to mean
            // the whole interface costs nothing, including the projection and the strings.
            return;
        }
        let started = Instant::now();
        let viewport = self.renderer.size();
        if self.scene.is_collision() {
            self.build_collision_ui();
            engine_presentation::ui::push_menu(
                &mut self.collision_ui,
                &self.menu,
                &self.menu_theme,
                &mut self.menu_scratch,
                viewport,
            );
            self.ui_ms = started.elapsed().as_secs_f64() * 1e3;
            self.renderer.set_ui(&self.collision_ui);
            return;
        }
        let config = NameplateConfig {
            cell_pixels: UI_TEXT_SCALE,
            glyph_advance_pixels: engine_renderer::font::GLYPH_ADVANCE as f32 * UI_TEXT_SCALE,
            line_advance_pixels: engine_renderer::font::LINE_ADVANCE as f32 * UI_TEXT_SCALE,
            glyph_height_pixels: engine_renderer::font::GLYPH_HEIGHT as f32 * UI_TEXT_SCALE,
            ..NameplateConfig::default()
        };

        let view_projection = self.camera.view_projection();
        self.nameplates
            .build(&self.simulation, &view_projection, viewport, &config);

        self.snapshot.refresh(&self.simulation);
        self.game_ui
            .build(&self.snapshot, self.nameplates.plates(), &config, viewport);
        // Last, so numbers float over the HUD rather than under it.
        self.game_ui.push_damage_numbers(
            &self.numbers,
            &view_projection,
            viewport,
            self.simulation.time() as f32,
        );
        // The tooltip for whatever the pointer is over, then the talent menu, then the
        // pause menu: painter's order is the whole sort.
        if let Some(ability) = self.hovered_ability {
            let (rect, view) = match ability.bar_slot() {
                Some(slot) => (
                    GameUi::slot_rect(slot, viewport),
                    self.snapshot.abilities[slot],
                ),
                None => (GameUi::ultimate_rect(viewport), self.snapshot.ultimate),
            };
            let ignite_max = self.snapshot.ignite_max;
            self.game_ui
                .push_ability_tooltip(rect, &view, ignite_max, viewport);
        }
        self.push_affected_highlights(viewport);
        engine_presentation::ui::talent_menu::push_talent_menu(
            self.game_ui.list_mut(),
            &self.talent_menu,
            &self.snapshot,
            &self.talent_theme,
            &mut self.talent_scratch,
            viewport,
        );

        // Last of all, into the same list: painter's order puts the menu over the HUD, and
        // sharing the list keeps the whole interface one batch and one draw call.
        engine_presentation::ui::push_menu(
            self.game_ui.list_mut(),
            &self.menu,
            &self.menu_theme,
            &mut self.menu_scratch,
            viewport,
        );
        self.ui_ms = started.elapsed().as_secs_f64() * 1e3;
        self.renderer.set_ui(self.game_ui.list());
    }

    /// Puts the area indicator on the ground, or takes it away.
    ///
    /// Two flat meshes a few centimetres up: a dim fill for the footprint and a bright ring
    /// for its edge, both scaled to the ability's own radius so what is shown is exactly
    /// what will be hit. The ring breathes slowly, which is what makes it read as a live
    /// indicator rather than as a decal somebody left on the floor.
    fn update_targeting_decal(&mut self, elapsed: f32) {
        let Some(target) = self.targeting.active() else {
            if let Some((fill, ring)) = self.targeting_decal.take() {
                self.extractor.render_world_mut().despawn(fill);
                self.extractor.render_world_mut().despawn(ring);
            }
            self.affected.clear();
            return;
        };

        let visuals = self.decal_visuals;
        let (fill_material, ring_material) = if target.valid {
            (visuals.fill_valid, visuals.ring_valid)
        } else {
            (visuals.fill_invalid, visuals.ring_invalid)
        };

        // A slow breath, and the ring sits a hair above the fill so the two never fight
        // over the same depth.
        let pulse = 1.0 + (elapsed * DECAL_PULSE_RATE).sin() * DECAL_PULSE_DEPTH;
        let radius = target.radius;
        let flat = |scale: f32, height: f32| engine_core::transform::Transform {
            translation: Vec3::new(target.position.x, height, target.position.z),
            rotation: engine_core::math::Quat::IDENTITY,
            scale: Vec3::new(scale, 1.0, scale),
        };
        let fill_transform = flat(radius * 2.0, DECAL_HEIGHT);
        let ring_transform = flat(radius * 2.0 * pulse, DECAL_HEIGHT + DECAL_RING_LIFT);
        let bounds = engine_core::math::Sphere::new(Vec3::ZERO, 0.75);

        let world = self.extractor.render_world_mut();
        match self.targeting_decal {
            Some((fill, ring)) => {
                world.set_transform(fill, fill_transform, bounds);
                world.set_material(fill, fill_material);
                world.set_transform(ring, ring_transform, bounds);
                world.set_material(ring, ring_material);
            }
            None => {
                let mut spawn = |mesh, material, transform| {
                    world.spawn(engine_render_data::RenderEntityDesc {
                        transform,
                        mesh,
                        material,
                        bounds,
                        visible: true,
                        style: engine_render_data::InstanceStyle::NEUTRAL,
                    })
                };
                let fill = spawn(visuals.fill_mesh, fill_material, fill_transform);
                let ring = spawn(visuals.ring_mesh, ring_material, ring_transform);
                self.targeting_decal = Some((fill, ring));
            }
        }

        // Everything the shape currently covers, so the interface can mark it. The shape
        // decides, not the app: a future cone marks what a cone covers with no change here.
        self.affected.clear();
        let facing = self
            .simulation
            .world()
            .body(self.simulation.player().entity)
            .map_or(0.0, |body| body.facing_yaw);
        let world = self.simulation.world();
        for slot in 0..world.slot_count() {
            let Some(id) = world.id_at(slot) else {
                continue;
            };
            if id == self.simulation.player().entity || !world.is_alive(id) {
                continue;
            }
            let Some(position) = world.position(id) else {
                continue;
            };
            if target.shape.contains(target.position, facing, position) {
                self.affected.push(id);
            }
        }
    }

    /// Rings whatever the targeting circle is currently covering.
    ///
    /// Drawn over the enemies themselves rather than only on the ground: a player placing a
    /// Meteor is choosing *who* it hits, and the answer should not have to be inferred from
    /// where a disc happens to overlap.
    fn push_affected_highlights(&mut self, viewport: (u32, u32)) {
        if self.affected.is_empty() {
            return;
        }
        let view_projection = self.camera.view_projection();
        for index in 0..self.affected.len() {
            let entity = self.affected[index];
            let Some(position) = self.simulation.world().position(entity) else {
                continue;
            };
            let height = self.simulation.world().height(entity);
            let Some(screen) = engine_presentation::nameplate::project(
                position + Vec3::new(0.0, height * 0.5, 0.0),
                &view_projection,
                viewport,
            ) else {
                continue;
            };
            let size = AFFECTED_MARK_SIZE;
            let rect = UiRect::new(screen.0 - size * 0.5, screen.1 - size * 0.5, size, size);
            self.game_ui
                .list_mut()
                .outline(rect, 3.0, [1.0, 0.72, 0.20, 0.95]);
        }
    }

    /// Builds the collision demo's interface: labels over the test geometry, nothing else.
    ///
    /// No health bars, no ability bar, no cast bar, no nameplates. The demo scene has no
    /// combat to report and the whole point of it is that nothing unrelated is in the way.
    fn build_collision_ui(&mut self) {
        self.collision_ui.clear();
        let Some(scene) = self.demo.as_ref() else {
            return;
        };
        #[cfg(feature = "debug-overlay")]
        collision_hud::push_markers(
            &mut self.collision_ui,
            &scene.markers,
            &self.camera.view_projection(),
            self.renderer.size(),
            self.controller.eye(),
            UI_TEXT_SCALE,
        );
        // Without the developer feature there is nothing to label: the geometry is not
        // drawn either, so a marker would point at empty space.
        #[cfg(not(feature = "debug-overlay"))]
        let _ = scene;
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
        let ui = self.renderer.ui_stats();
        log::info!(
            "CPU mean {:.3} ms ({:.0} FPS) p99 {:.3} | GPU {} | sim {:.3} ms ({} ticks) | \
             extract {:.3} ms | cull {:.3} ms | ui {:.3} ms ({} quads, {} cmds) | visible {} | \
             draws {}",
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
            self.ui_ms + self.renderer.ui_build_ms(),
            ui.quads,
            ui.commands,
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

        // The same readout in both scenes: it reports the frame rate, and the frame rate
        // does not depend on which scene is loaded. Everything scene-specific — collision
        // state, the combat meter, cull and draw counts — is still measured and still
        // reported by the periodic log line and by `engine-bench`; it is simply not drawn
        // over the game any more.
        let model = HudModel {
            summary: ctx.metrics().summary(),
            sim: self.simulation.last_tick_stats(),
            ticks_this_frame: self.simulation.stats().ticks,
            tick_hz: self.simulation.timestep().hz(),
            memory: Some(ctx.frame_allocator().stats()),
        };
        hud::write_lines(&mut self.overlay_lines, &model);
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
        // p50 as well as the tail: a mean hides a spike that a p50/p99 pair makes obvious,
        // and Phase 4's budgets are stated against p99 rather than the mean.
        log::info!(
            "final CPU window: mean {:.3} | p50 {:.3} | p95 {:.3} | p99 {:.3} | max {:.3} ms | n={}",
            summary.cpu.mean_ms,
            summary.cpu.p50_ms,
            summary.cpu.p95_ms,
            summary.cpu.p99_ms,
            summary.cpu.max_ms,
            summary.cpu.count
        );
        if let Some(gpu) = summary.gpu {
            log::info!(
                "final GPU window: mean {:.3} | p50 {:.3} | p95 {:.3} | p99 {:.3} | max {:.3} ms \
                 | n={}",
                gpu.mean_ms,
                gpu.p50_ms,
                gpu.p95_ms,
                gpu.p99_ms,
                gpu.max_ms,
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
                // Movement is accumulated from the messages, not from a position compared
                // against the centre once a frame: the re-centring warp produces a message
                // of its own, and only here can it be told apart from the player's hand.
                WindowEvent::Input(InputEvent::MouseMoved { x, y }) => {
                    self.motion.moved((x, y));
                }
                WindowEvent::Input(InputEvent::FocusChanged { focused: false }) => {
                    // The cursor's hidden state is process-wide; leaving it hidden while
                    // another window has focus takes the pointer away from the desktop.
                    self.cancel_mouse_look();
                }
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
            // The menu sees the frame's input first, and then gameplay sees it too: the
            // world is shared, so a player with a settings screen open is still standing in
            // it and still has to be able to move out of the way.
            self.update_menu(ctx);
            if !self.menu.is_open() {
                self.update_firemage_ui(ctx);
            }
            self.update_targeting_decal(ctx.elapsed().as_secs_f32());

            if !self.targeting.is_active() && !self.talent_menu.is_open() {
                self.apply_mouse_look(ctx);
            }
            let wheel = ctx.input().wheel_delta();
            if wheel != 0.0 {
                self.controller.zoom(wheel);
            }
            self.commands.clear();
            // The collision scene issues movement and jump only: it has no abilities and no
            // targets, and its digits stand the character at a test station instead.
            if self.scene.is_collision() {
                self.bindings.write_movement_commands(
                    ctx.input(),
                    self.controller.yaw,
                    &mut self.commands,
                );
            } else {
                self.bindings
                    .write_commands(ctx.input(), self.controller.yaw, &mut self.commands);
            }
            self.simulation.push_commands(self.commands.iter().copied());
        }

        {
            engine_core::profile_scope!(profiler, "Simulation");
            // The *real* delta, always: the fixed timestep is what decouples gameplay from
            // the frame rate, and feeding it a scaled or clamped delta would do that job
            // twice. There is no menu case here — a shared world does not stop because one
            // player opened a settings screen.
            self.simulation.advance(ctx.time().real_delta());
        }

        {
            engine_core::profile_scope!(profiler, "Extraction");
            let started = Instant::now();
            // Damage becomes a hit flash by *observing* the event stream, exactly as the
            // damage meter does. Nothing in the ability system knows a flash exists, and
            // gameplay is not asked to remember that something was hit.
            let time = self.simulation.time() as f32;
            for event in self.simulation.events() {
                if let engine_game::event::CombatEvent::Damage(damage) = event {
                    self.extractor.note_hit(damage.target, time);
                }
            }
            // Damage numbers read the same slice. Both are observers; neither is a hook the
            // ability system calls, which is what keeps gameplay unaware of either.
            let world = self.simulation.world();
            self.numbers.observe(self.simulation.events(), time, |id| {
                world.position(id).map(|position| {
                    // Chest height, so a number starts on the body rather than at its feet.
                    position + Vec3::new(0.0, world.height(id) * 0.7, 0.0)
                })
            });
            self.numbers.retire(time);
            self.extractor.extract(&self.simulation);
            // After extraction, because it spawns into the same `RenderWorld` and would
            // otherwise be drawing last frame's collision state beside this frame's
            // character.
            #[cfg(feature = "debug-overlay")]
            self.collision_debug
                .rebuild(self.extractor.render_world_mut(), &self.simulation);
            self.extract_ms = started.elapsed().as_secs_f64() * 1e3;
        }

        {
            engine_core::profile_scope!(profiler, "Camera");
            // The render delta, not the simulation step: the camera is presentation and is
            // expected to move every frame, which is what stops a 60 Hz simulation from
            // looking like 60 FPS.
            // The *interpolated* position, so the camera and the character it watches
            // agree about where the character is. Following the raw simulation position
            // would step the camera at 60 Hz behind a character moving smoothly.
            self.controller.follow(
                engine_presentation::extract::interpolated_player_position(&self.simulation),
                ctx.time().real_delta().as_secs_f32(),
            );
            let (width, height) = self.renderer.size();
            if width > 0 && height > 0 {
                self.controller
                    .apply(&mut self.camera, aspect_of(width, height))?;
            }
        }

        // After the camera, because a nameplate is a projection through it: building the UI
        // from last frame's camera lags it a frame behind the enemies it labels.
        {
            engine_core::profile_scope!(profiler, "UI");
            self.build_ui();
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
        self.cancel_mouse_look();
        // A value changed by the keyboard and never released, or a write that failed
        // earlier: the session's last chance to get it on disk.
        if self.settings.save_if_dirty() {
            log::info!("settings: saved on shutdown");
        }
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

/// The meshes and materials the ground indicator is drawn with.
#[derive(Debug, Clone, Copy)]
struct DecalVisuals {
    /// A unit disc, scaled to the ability's radius.
    fill_mesh: MeshHandle,
    /// A unit ring, scaled the same way.
    ring_mesh: MeshHandle,
    /// The footprint of a valid placement.
    fill_valid: MaterialHandle,
    /// Its edge.
    ring_valid: MaterialHandle,
    /// The footprint of a placement that would be refused.
    fill_invalid: MaterialHandle,
    /// Its edge.
    ring_invalid: MaterialHandle,
}

/// Uploads the ground indicator's geometry and colours.
///
/// The fill is deliberately dark and the ring deliberately bright. The world pass is opaque,
/// so "translucent" has to be expressed as a shade the ground reads through rather than as an
/// alpha the renderer would ignore; the bright edge is what carries the shape.
fn upload_decal_visuals(renderer: &mut Renderer) -> Result<DecalVisuals, EngineError> {
    // Unit shapes: the transform scales them to whatever radius the ability has.
    let fill_mesh = renderer.create_mesh(&MeshData::disc(0.5, 48))?;
    let ring_mesh = renderer.create_mesh(&MeshData::ring(0.5, 0.035, 48))?;

    let fill_valid = renderer.create_material(
        MaterialDesc::colored(0.35, 0.16, 0.05)
            .with_roughness(1.0)
            .with_emissive(Vec3::new(0.16, 0.06, 0.01)),
    )?;
    let ring_valid = renderer.create_material(
        MaterialDesc::colored(1.0, 0.62, 0.18)
            .with_roughness(0.4)
            .with_emissive(Vec3::new(0.95, 0.45, 0.10)),
    )?;
    let fill_invalid = renderer.create_material(
        MaterialDesc::colored(0.32, 0.06, 0.06)
            .with_roughness(1.0)
            .with_emissive(Vec3::new(0.16, 0.02, 0.02)),
    )?;
    let ring_invalid = renderer.create_material(
        MaterialDesc::colored(0.95, 0.20, 0.18)
            .with_roughness(0.4)
            .with_emissive(Vec3::new(0.80, 0.08, 0.06)),
    )?;

    Ok(DecalVisuals {
        fill_mesh,
        ring_mesh,
        fill_valid,
        ring_valid,
        fill_invalid,
        ring_invalid,
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
fn log_controls(args: &Args, demo: Option<&DemoScene>) {
    log::info!("scene: {}", args.scene.as_str());
    match demo {
        Some(scene) => {
            log::info!(
                "collision demo: {} test stations, no combat",
                scene.stations.len()
            );
            for station in &scene.stations {
                log::info!(
                    "  {}  {:<12} {}",
                    station.key,
                    station.name,
                    station.instruction
                );
            }
        }
        None => log::info!("training arena: 1 dummy plus a group of {}", args.dummies),
    }

    log::info!("controls:");
    log::info!("  W A S D          move, relative to the camera");
    log::info!("  SPACE            jump");
    if demo.is_some() {
        log::info!("  1 - 7            stand at a test station");
        log::info!("  R                back to the first station");
    } else {
        log::info!("  1 - 0            firebolt, flame lance, flame wave, meteor,");
        log::info!("                   firestorm, pyroblast, inferno, fire detonation,");
        log::info!("                   flame shield, blink");
        log::info!("  R                cataclysm, the ultimate");
        log::info!("  4 5 R            place with the mouse: left click confirms, right cancels");
        log::info!("  TAB              cycle target");
        log::info!("  R                reset the dummies and the combat meter");
    }
    log::info!("  ESC              menu: settings, quit, resume (the game keeps running)");
    log::info!("  RIGHT MOUSE      hold to turn the camera; release to stop");
    log::info!("  MOUSE WHEEL      camera distance");
    log::info!("  F3               collision debug drawing on/off");
    log::info!("  F5               developer overlay on/off");
    log::info!("  F9               wireframe on/off");
}

/// Uploads the meshes and materials the collision debug view draws with.
///
/// A cube and a sphere, and three unlit-looking materials. Separate from
/// [`upload_visuals`] and behind the developer feature, so a shipping build uploads
/// neither the sphere nor the materials.
#[cfg(feature = "debug-overlay")]
fn upload_debug_visuals(renderer: &mut Renderer) -> Result<DebugBindings, EngineError> {
    let cube = MeshData::cube(1.0);
    // 16 segments and 8 rings: a capsule cap, not a planet. It is drawn twice.
    let sphere = MeshData::uv_sphere(1.0, 16, 8);
    let box_bounds = cube.bounds();
    let sphere_bounds = sphere.bounds();

    let box_mesh = renderer.create_mesh(&cube)?;
    let sphere_mesh = renderer.create_mesh(&sphere)?;

    // Emissive, because a debug shape has to read the same whichever way the sun is
    // pointing: a collision box lit like the world would be hard to tell from the world.
    let shape_material = renderer.create_material(
        MaterialDesc::colored(0.30, 0.75, 1.00)
            .with_roughness(0.8)
            .with_emissive(Vec3::new(0.06, 0.16, 0.22)),
    )?;
    let capsule_material = renderer.create_material(
        MaterialDesc::colored(0.40, 1.00, 0.55)
            .with_roughness(0.6)
            .with_emissive(Vec3::new(0.10, 0.25, 0.12)),
    )?;
    let contact_material = renderer.create_material(
        MaterialDesc::colored(1.00, 0.85, 0.30)
            .with_roughness(0.4)
            .with_emissive(Vec3::new(0.45, 0.35, 0.10)),
    )?;

    Ok(DebugBindings {
        box_mesh,
        box_bounds,
        sphere_mesh,
        sphere_bounds,
        shape_material,
        capsule_material,
        contact_material,
    })
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
