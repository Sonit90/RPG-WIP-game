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

use engine_assets::procedural::{
    DummyVariant, druid, druid_character, fire_mage, fire_mage_character, training_dummy, warrior,
    warrior_character,
};
use engine_core::application::{Application, EngineContext};
use engine_core::config::EngineSettings;
use engine_core::error::EngineError;
use engine_core::input::{InputEvent, KeyCode};
use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::demo::{self, DemoScene};
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::input_map::Bindings;
use engine_game::sim::Simulation;
use engine_platform::{Window, WindowConfig, WindowEvent};
use engine_presentation::AudioDirector;
use engine_presentation::camera::ThirdPersonCamera;
use engine_presentation::character::CharacterVisual;
#[cfg(feature = "debug-overlay")]
use engine_presentation::collision_debug::{CollisionDebug, DebugBindings};
#[cfg(feature = "debug-overlay")]
use engine_presentation::collision_hud;
use engine_presentation::damage_numbers::FloatingNumbers;
use engine_presentation::extract::{RenderExtractor, ViewParams, VisualBindings};
use engine_presentation::game_ui::GameUi;
#[cfg(feature = "debug-overlay")]
use engine_presentation::hud::{self, HudModel};
use engine_presentation::mouse_look::{LookChange, LookMode, MouseLook, RelativeMotion};
use engine_presentation::nameplate::{NameplateConfig, NameplateSet};
#[cfg(feature = "debug-overlay")]
use engine_presentation::shadow_debug::ShadowCasterDebug;
use engine_presentation::snapshot::UiSnapshot;
use engine_presentation::ui::action_bar::{self, ActionBarSet, SlotAddress};
use engine_presentation::ui::character_menu::{
    CharacterMenu, CharacterMenuOutcome, CharacterTheme, push_character_menu,
};
use engine_presentation::ui::ground_decal::{DecalBand, GroundSurface};
use engine_presentation::ui::hud_edit::{EditOutcome, HudEditor, push_hud_editor};
use engine_presentation::ui::hud_layout::{HudElementId, HudLayout};
use engine_presentation::ui::slot_menu::{SlotMenu, SlotMenuOutcome, push_slot_menu};
use engine_presentation::ui::talent_menu::{TalentMenu, TalentMenuOutcome, TalentTheme};
use engine_presentation::ui::targeting::{GroundTargeting, TargetingOutcome};
use engine_presentation::ui::unit_frame::FrameScratch;
use engine_presentation::ui::{
    MenuAction, MenuScratch, MenuSystem, MenuTheme, UiInput, VolumeChannel,
};
use engine_presentation::vfx::{VfxDirector, VfxVisuals};
use engine_render_data::ui::{UiRect, UiScale};
use engine_renderer::mesh::MeshData;
use engine_renderer::{
    Camera, Lighting, MaterialDesc, MaterialHandle, MeshHandle, Projection, Renderer,
    RendererConfig, ShadowPreset, UiDrawList,
};
use engine_settings::character::CharacterConfigManager;
use engine_settings::hud::{BindingScope, HudConfigManager};
use engine_settings::{NameplateSetting, SettingsManager};

use crate::args::{Args, Scene};
use crate::audio_device::AudioHost;
use crate::benchmark::{Action, Boundary, FrameReport, ResolutionBenchmark};

/// Clear colour of the back buffer: a dark blue-grey that reads as sky.
const CLEAR_COLOR: [f32; 4] = [0.07, 0.09, 0.13, 1.0];

/// Half-width of the visible floor, in metres.
///
/// Larger than the arena the character is confined to, so the world does not end at the
/// wall the player runs into.
const GROUND_HALF_EXTENT: f32 = 120.0;

/// How aggressively levels of detail are dropped.
///
/// 1.0 is the thresholds the models were authored with. A quality setting would move this;
/// nothing exposes it yet, and it is a named constant rather than a literal at the call site
/// so the knob is findable when something does.
const LOD_BIAS: f32 = 1.0;

/// Dummies the VFX test bed spawns, to aim effects at and to judge their scale against.
const VFX_SCENE_DUMMIES: usize = 3;

/// Dummies the audio test bed spawns. The same three, for the same reason: a sound at a
/// place is only judgeable against something that is visibly at that place.
const AUDIO_SCENE_DUMMIES: usize = 3;

/// How far in front of the player the audio test bed drops a sound, in metres.
///
/// Far enough that walking towards and away from it is what changes its level — which is
/// the whole point of dropping it at a place instead of playing it on the player.
const AUDIO_DEMO_DISTANCE: f32 = 8.0;

/// How far in front of the player the VFX test bed plays an effect, in metres.
const VFX_DEMO_DISTANCE: f32 = 6.0;

/// Radius the VFX test bed plays area effects at, in metres.
///
/// A middling arena radius rather than any one ability's, because the point of the scene is
/// judging the *effect*, and every ability scales the same effect by its own number.
const VFX_DEMO_RADIUS: f32 = 5.0;

/// Function key that toggles the developer overlay.
#[cfg(feature = "debug-overlay")]
const OVERLAY_TOGGLE_KEY: KeyCode = KeyCode::F5;

/// Function key that toggles the collision debug drawing.
///
/// F3 by the project's existing convention of putting developer toggles on the function
/// row beside F5 (overlay) and F9 (wireframe).
#[cfg(feature = "debug-overlay")]
const COLLISION_DEBUG_KEY: KeyCode = KeyCode::F3;

/// Function key that cycles shadow quality: off, low, medium, high, ultra, off.
///
/// A developer key on the same row, *and* the setting the player would pick on a settings
/// screen — the key writes the same field the file holds, so what it changes is what is
/// saved. Changing it stalls for a GPU idle, which is why it is a key press rather than
/// something a slider fires per frame.
///
/// F4 rather than F6: F6 opens and closes the level editor in a development build, and one
/// press must not both open the editor and stall the GPU to rebuild the shadow atlas. The
/// two are read in different places — the editor reads the input state, this is a key event —
/// so a shared key is answered twice rather than once.
const SHADOW_QUALITY_KEY: KeyCode = KeyCode::F4;

/// Function key that cycles the debug views: the shadow ones, then the material ones.
///
/// §18 asks for a visualization that needs no change to game content. This is it: the mesh
/// shader already has every number the view needs, so a debug mode is a constant in the
/// frame buffer rather than a second pass.
///
/// It was F7 until the player lifecycle needed that key. Moved rather than shared: a key that
/// kills the player *and* cycles a shader constant is a key nobody can press with confidence.
const SHADOW_DEBUG_KEY: KeyCode = KeyCode::F10;

/// Function key that kills the player, for testing death and respawn.
///
/// **A developer control.** It is not a gameplay mechanic and nothing in the game binds it:
/// it goes straight to `Simulation::kill_player`, which goes through the ordinary damage
/// path, so what follows is the shipping death and the shipping respawn rather than a second
/// path that only this key can reach. That is the whole point of it — a debug key that took a
/// shortcut would be a debug key that tests nothing.
const KILL_PLAYER_KEY: KeyCode = KeyCode::F7;

/// Which of the generated materials the terrain is drawn with.
///
/// Named rather than chosen per level, because a blockout terrain is one surface and picking
/// its material per level is a decision nobody has needed to make yet. When they do, it
/// becomes a field on the terrain and this becomes its default.
const TERRAIN_MATERIAL: &str = "grass";

/// Which of the generated materials a body of water is drawn with.
///
/// `"water"` is the only transparent entry in the starter library, and one surface serves the
/// whole level for the reason the terrain's does: when somebody needs two, it becomes a field
/// on the body rather than a second constant here.
const WATER_MATERIAL: &str = "water";

/// How wide the targeting indicator's border is, in metres.
///
/// Wide enough to read as a drawn line at the gameplay camera's distance, narrow enough that
/// it marks a boundary rather than covering ground either side of it. Its *outer* rim is the
/// ability's edge, so the line the player aims with is the boundary the executor tests.
const DECAL_BORDER_WIDTH: f32 = 0.28;

/// How far above the ground the border sits, in metres.
///
/// Coplanar with the terrain would be a depth-buffer tie and would flicker; a few centimetres
/// reads as painted on the ground rather than hovering over it, and is far below what the eye
/// resolves at the camera's distance.
const DECAL_BORDER_LIFT: f32 = 0.05;

/// How wide the translucent band inside the border is, in metres.
///
/// The whole of the indicator's interior: everything further in is bare terrain. A band is
/// what makes the enclosed area read as enclosed without an opaque disc hiding the ground the
/// player is aiming at.
const DECAL_GLOW_WIDTH: f32 = 0.85;

/// How far above the ground the translucent band sits, in metres.
///
/// Below the border, so the two never tie for depth where they meet.
const DECAL_GLOW_LIFT: f32 = 0.035;

/// How far the cursor may travel during a left press and still count as a click.
///
/// The whole difference between "select what I am pointing at" and "look around". Generous
/// enough that a hand that is not perfectly still still selects, small enough that a
/// deliberate camera turn never does — a turn moves hundreds of pixels, not six.
const CLICK_SLOP_PIXELS: i32 = 6;

/// Size of the ring drawn over an enemy inside the targeting circle, in pixels.
const AFFECTED_MARK_SIZE: f32 = 46.0;

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
    /// What the back buffer is cleared to. The scene's, where there is a scene.
    clear_color: [f32; 4],
    /// The environment, spawned into the extractor's `RenderWorld` once and kept at the
    /// right level of detail. `None` for the two scenes that bring no environment.
    ///
    /// It owns render entities in a world the extractor also writes to, and that is
    /// deliberate: two worlds would be two culls, two instance buffers and two passes for
    /// objects that share a frustum and half their materials with the fight in front of them.
    environment: Option<engine_environment::EnvironmentScene>,
    placer: engine_presentation::EnvironmentPlacer,
    /// The render entity the level's terrain is drawn as, if it has one.
    ///
    /// One entity for the whole surface, respawned when the heights change. Not an
    /// `EnvironmentObject`, because a terrain is not a placement: it has no asset id, no
    /// transform an author can drag, and exactly one of it per level.
    terrain_entity: Option<engine_render_data::RenderEntity>,
    /// One render entity per body of water the level holds.
    ///
    /// Respawned together when any body changes, for the reason the terrain's single entity
    /// is: a level has a handful of them, and rebuilding all of them costs less than working
    /// out which one an author just dragged.
    water_entities: Vec<engine_render_data::RenderEntity>,
    /// The markers drawn over the level's spawn points, while the editor is open.
    ///
    /// Editor furniture, and empty in a build without one: a spawn point has no mesh, and the
    /// thing an author clicks on is this. They are despawned on the way out of the editor, so
    /// a player never sees a signpost standing where they are about to appear.
    #[cfg(feature = "editor")]
    spawn_marker_entities: Vec<engine_render_data::RenderEntity>,
    bindings: Bindings,
    /// Which scenario is loaded. Chosen once, at startup, by `--scene`.
    scene: Scene,
    /// The level editor, in a build that has one.
    ///
    /// Behind the `editor` feature, so a player's binary has no field here at all — the
    /// editor is development scaffolding and must not ship inside the game.
    #[cfg(feature = "editor")]
    editor: engine_editor::Editor,
    /// Whether the editor should open itself on the next frame, from `--editor`.
    ///
    /// A request rather than a call, because opening needs a viewport and a camera that the
    /// constructor does not have yet: the first frame answers it through exactly the same
    /// path F6 goes down, so "started in the editor" and "pressed F6" cannot drift apart.
    #[cfg(feature = "editor")]
    editor_open_requested: bool,
    /// Where the editor saves, when `--scene-file` named a file.
    ///
    /// Without one the editor names its own file after the scene. With one, CTRL+S writes
    /// back to the file the level was opened from, which is what makes opening, editing and
    /// saving a level one path rather than a save-as every time.
    #[cfg(feature = "editor")]
    editor_save_path: Option<std::path::PathBuf>,
    /// The collision demo's stations and labels. `None` in the combat scene.
    demo: Option<DemoScene>,
    /// The collision demo's world-space labels. Its own draw list, because the demo scene
    /// draws none of the combat interface.
    collision_ui: UiDrawList,
    /// The collision world, drawn. Developer-only, and off in the combat scene.
    #[cfg(feature = "debug-overlay")]
    collision_debug: CollisionDebug,
    /// Which stop of the F7 cycle is showing. 0-3 are the renderer's shader views, 4 is the
    /// caster bounds, which are geometry rather than a shader mode.
    shadow_debug_view: u32,
    /// The shadow caster bounds, drawn. The fourth stop on the F7 cycle.
    #[cfg(feature = "debug-overlay")]
    shadow_debug: ShadowCasterDebug,
    /// Commands built from this frame's input. Reused; never freed.
    commands: Vec<PlayerCommand>,
    /// Every playable hero's model, uploaded once at startup.
    ///
    /// All three are uploaded whether or not they are worn, because a swap happens from a
    /// menu in a running frame: uploading a model there would mean a `create_model` — a
    /// buffer allocation and a copy — in the middle of a frame the player is looking at, to
    /// save perhaps a megabyte of vertex data. Three prism characters are cheaper than one
    /// hitch.
    heroes: HeroVisuals,
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
    /// Where every arrangeable interface element sits. The one source of HUD geometry.
    hud_layout: HudLayout,
    /// The HUD layout's file. Separate from `settings` on purpose: a dragged element
    /// rewrites this and has no business rewriting a volume.
    hud_config: HudConfigManager,
    /// The eight action bars: what is in each slot and which key presses it.
    action_bars: ActionBarSet,
    /// Edit HUD: off unless the player asked for it from the pause menu.
    hud_editor: HudEditor,
    /// The menu a right click on an action bar slot opens.
    slot_menu: SlotMenu,
    /// Scratch the frames and the slot menu format their text into. Reused; never freed.
    frame_scratch: FrameScratch,
    /// Scratch the HUD's own labels are written into. Reused; never freed.
    hud_scratch: String,
    /// The one ground-targeting circle, shared by Meteor, Firestorm and Cataclysm.
    targeting: GroundTargeting,
    /// The talent menu, opened from the button in the top-right corner.
    talent_menu: TalentMenu,
    /// How the talent menu is coloured.
    talent_theme: TalentTheme,
    /// Scratch the talent menu formats its numbers into. Reused; never freed.
    talent_scratch: String,
    /// The character sheet, opened from the corner button or the character key.
    character_menu: CharacterMenu,
    /// How the character sheet is coloured.
    character_theme: CharacterTheme,
    /// Scratch the character sheet formats its numbers into. Reused; never freed.
    character_scratch: String,
    /// What each character wears and carries, and the file it lives in.
    character_config: CharacterConfigManager,
    /// Which ability button the pointer is over, for the tooltip.
    hovered_ability: Option<FireAbility>,
    /// Which action bar slot that ability is in, when the pointer is over a bar.
    ///
    /// Remembered rather than searched for again: the same spell may sit in several slots on
    /// several bars, and a tooltip that looked its ability up would point at the first copy
    /// instead of the one under the pointer.
    hovered_slot: Option<SlotAddress>,
    /// The area indicator drawn on the ground while an ability is being placed.
    ///
    /// Two render entities rather than a screen-space circle: an area on the floor has to
    /// lie *on the floor*, or the player cannot tell what it will cover. The fill is the
    /// shaded footprint and the ring is its bright edge, which is what makes the shape
    /// readable over grass, stone or a corpse.
    targeting_decal: Option<TargetingDecal>,
    /// Scratch for the indicator's outline, reused every frame.
    ///
    /// The indicator is rebuilt whenever the cursor moves, which is most frames a player
    /// spends aiming; owning the buffers here is what makes that rebuild allocation-free.
    decal_scratch: engine_presentation::ui::ground_decal::DecalScratch,
    /// Scratch for the border band's piece transforms.
    decal_border: Vec<engine_core::transform::Transform>,
    /// Scratch for the translucent band's piece transforms.
    decal_glow: Vec<engine_core::transform::Transform>,
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
    /// The press that might still turn out to be a click, and where it landed.
    ///
    /// `Some` from the moment the orbit button goes down over the world until it comes back
    /// up. `None` while the press belonged to the interface, or while the right button is
    /// steering — a click selects, and only the left button clicks.
    click_candidate: Option<(i32, i32)>,
    /// Cursor pixels travelled since [`App::click_candidate`] was set.
    ///
    /// What separates a click from a drag. A press that moved the camera was the player
    /// looking around, and selecting whatever happened to be under the cursor when they
    /// stopped would be a selection they did not ask for.
    click_travel: i32,
    /// Milliseconds the last render extraction took.
    ///
    /// Extraction alone: the VFX update is timed separately, because "what does the fight
    /// cost to extract" and "what do its effects cost to advance" are different questions
    /// and one number for both hides which of them moved.
    extract_ms: f64,
    /// Milliseconds the last VFX observation and update took.
    vfx_ms: f64,
    /// Milliseconds the last UI *layout* took: nameplates, snapshot and draw-list build.
    ///
    /// The renderer measures the batching that follows it separately, so the two together
    /// are the whole CPU cost of the interface.
    ui_ms: f64,
    /// This frame's nameplates. Reused; never freed.
    nameplates: NameplateSet,
    /// How long the last frame took, in seconds of real time.
    ///
    /// Kept on the application because the UI is built after the camera, in a stage that does
    /// not carry the engine's context — and the nameplate layout needs a time step to smooth
    /// with. Real time, not simulated: this is how fast the *screen* is updating.
    frame_seconds: f32,
    /// What the UI is allowed to know about the simulation, rebuilt each frame.
    snapshot: UiSnapshot,
    /// The game UI's layout. Owns its draw list across frames.
    game_ui: GameUi,
    /// Floating damage numbers in flight. Fixed capacity; never freed.
    numbers: FloatingNumbers,
    /// Combat, drawn. Observes the same event stream the numbers and the meter read.
    vfx: VfxDirector,
    /// The audio device and the runtime it mixes. Silent on a machine with no sound card,
    /// and silent on purpose with `--no-audio`.
    audio: AudioHost,
    /// Combat turned into sound: the audio twin of `vfx`, observing the same events.
    audio_director: AudioDirector,
    /// What this frame's cues and listener update cost, in milliseconds.
    ///
    /// The *game thread's* share only. Mixing happens on the device's own thread and is
    /// measured by `engine-bench`, because a frame does not wait for it.
    audio_ms: f64,
    /// Which sound the audio test bed has selected. Zero in every other scene.
    audio_demo_selection: usize,
    /// Effects a second the stress driver plays, or zero when it is off.
    vfx_stress: f32,
    /// Effects the stress driver owes, carried between frames.
    vfx_stress_debt: f32,
    /// Which catalogue effect it plays next, so the load covers the whole catalogue.
    vfx_stress_next: usize,
    /// Which catalogue effect the VFX test bed last played, for its log line.
    #[cfg(feature = "debug-overlay")]
    last_effect: Option<engine_vfx::VfxId>,
    report_interval: Option<Duration>,
    /// Whether the window should ever be shown. False for `--hidden`.
    show_window: bool,
    /// Whether it has been shown yet. The first presented frame flips this.
    window_shown: bool,
    /// Frames discarded from the statistics window before it starts collecting.
    ///
    /// Zero for an ordinary run. A benchmark passes `--warmup` so that driver warm-up,
    /// pipeline creation and the first uploads - all of which happen in the opening frames
    /// and none of which happen again - stay out of the percentiles.
    warmup_frames: u64,
    /// Individual shadow fields `--shadow-tweak` named, applied over whichever preset is
    /// current - including the one F4 switches to.
    shadow_overrides: crate::args::ShadowOverrides,
    /// When the measured window opened, and at which frame.
    ///
    /// The run's own average frame rate covers the warm-up too, and at 900 frames the
    /// opening tenth of a second is a large enough share of it to move the number. The
    /// steady-state rate is measured from here instead.
    measured_from: Option<(Instant, u64)>,
    /// The resolution matrix, when `--benchmark` asked for one.
    ///
    /// It drives the window: each stage asks for a size, waits for the renderer to report it,
    /// discards warm-up frames and then measures. One process, one scene, one set of GPU
    /// resources — see `crate::benchmark`.
    benchmark: Option<ResolutionBenchmark>,
    /// The shadow stage's own GPU time, one sample per frame that resolved a query.
    ///
    /// The frame's total says the frame got slower; this says whether the shadow stage is
    /// where it went. Kept as a window rather than a running mean for the same reason the
    /// frame's is: a mean hides a stall and a p50 does not.
    shadow_gpu: engine_core::time::FrameStatsWindow,
    last_report: Instant,
    /// Scratch buffer the periodic report is built into, kept between reports so the
    /// steady state allocates nothing.
    report: String,
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

        // Created **hidden**, whatever the caller asked for, and shown after the first
        // frame has been presented (see `present`). Between `CreateWindowExW` and the first
        // `Present` the client area holds whatever the compositor last had there, which on
        // Windows is white — and loading a scene takes long enough for that to be a white
        // flash a player sees. Showing the window only once there is a frame in it removes
        // the flash rather than recolouring it.
        let window_config = WindowConfig::new(&settings.title, settings.width, settings.height)
            .with_fullscreen(settings.display_mode.is_fullscreen())
            .with_activate(args.focus)
            .with_visible(false);
        let window = Window::new(&window_config)?;
        crate::startup::stage("window init");
        if !args.window {
            log::info!("window: --hidden; the game runs and renders with nothing on screen");
        }

        // The window is the authority on its client size: fullscreen takes the monitor's
        // size, and the OS may clamp a requested size on a small display.
        let (width, height) = window.client_size();
        let mut renderer_config = RendererConfig::new(width, height).with_vsync(settings.vsync);
        renderer_config.enable_debug_layer = settings.renderer_validation;
        renderer_config.break_on_validation_error =
            settings.renderer_validation && cfg!(debug_assertions);
        let mut renderer = Renderer::new(window.raw_handle(), &renderer_config)?;
        {
            // The renderer measured its own two halves; this puts them in the same table as
            // everything else rather than in a second one.
            let timings = renderer.startup();
            crate::startup::stage_from(
                "D3D12 device",
                Instant::now() - Duration::from_secs_f64(timings.device_ms / 1e3),
            );
            crate::startup::stage_from(
                "renderer objects",
                Instant::now() - Duration::from_secs_f64(timings.objects_ms / 1e3),
            );
            crate::startup::stage("renderer init");
        }
        #[cfg(feature = "debug-overlay")]
        renderer.set_overlay_enabled(settings.debug_overlay);

        renderer.set_ui_enabled(args.ui);

        // Every resource the level needs goes up in one batch: one submission and one wait
        // for the GPU at the end of it, rather than a full round trip per texture and per
        // buffer. See `Renderer::begin_upload_batch` for what that was costing.
        renderer.begin_upload_batch()?;
        let visuals = upload_visuals(&mut renderer)?;
        let decal_visuals = upload_decal_visuals(&mut renderer)?;
        let vfx_visuals = upload_vfx_visuals(&mut renderer)?;
        #[cfg(feature = "debug-overlay")]
        let debug_visuals = upload_debug_visuals(&mut renderer)?;
        crate::startup::stage("GPU upload: shared");

        // The level. Built before the simulation, because the simulation's arena bounds and
        // spawn points come out of it — the scene owns where there is room to stand, because
        // the scene is what put a boulder somewhere.
        let authored = load_authored_scene(args)?;
        let environment = authored.or_else(|| {
            args.scene.has_environment().then(|| {
                // A density above one is the stress layout: the same boundary, the same spawn
                // points and several times the scatter. Loading it through the ordinary path
                // rather than through a separate scene is deliberate — a stress configuration
                // that goes down a different code path measures the other code path.
                if args.scene.is_dungeon() {
                    // The same deterministic function gameplay built its collision from. Building
                    // the level twice is how the picture and the collision end up describing two
                    // different rooms.
                    engine_game::dungeon::dungeon_scene()
                } else if args.env_density > 1.0 {
                    engine_environment::arena::stress_arena(args.env_density)
                } else if args.scene.is_line_of_sight() {
                    // The same deterministic layout gameplay built its collision from, for the
                    // reason the dungeon does it: two descriptions of one course are two courses.
                    engine_environment::los_course::los_course()
                } else if args.scene.is_shadows() {
                    engine_environment::shadow_course::shadow_course()
                } else if args.scene.is_environment() {
                    engine_environment::arena::environment_demo()
                } else {
                    engine_environment::arena::combat_arena()
                }
            })
        });

        // The lighting stress control, applied to whichever scene loaded rather than to one
        // of them: "what do many lights cost" is a question about the renderer, and a knob
        // that only worked in the arena would answer it for the arena.
        let environment = environment.map(|mut scene| {
            engine_environment::push_stress_lights(
                &mut scene,
                args.stress_lights,
                args.stress_shadow_lights,
            );
            scene
        });
        if args.stress_lights > 0 {
            log::info!(
                "lighting: {} stress light(s) added, {} of them casting",
                args.stress_lights,
                args.stress_shadow_lights.min(args.stress_lights),
            );
        }

        crate::startup::stage("scene loading");

        let (mut simulation, demo) = match args.scene {
            Scene::Combat => (
                match environment.as_ref() {
                    Some(scene) => arena::environment_arena(scene, args.dummies),
                    None => arena::training_arena(&ArenaConfig::with_group(args.dummies)),
                },
                None,
            ),
            // The environment test bed: the arena, with no fight in it. One dummy remains,
            // because a scene with nothing man-sized in it gives the eye no scale reference
            // and every judgement about how big a pillar looks is then a guess.
            Scene::Environment => (
                match environment.as_ref() {
                    Some(scene) => arena::environment_arena(scene, 0),
                    None => arena::training_arena(&ArenaConfig::with_group(0)),
                },
                None,
            ),
            // The character test bed is the arena with the fight taken out: one dummy to
            // judge scale against, and no group to clutter the view. It is deliberately the
            // same simulation rather than a bespoke one, so the character being inspected is
            // the character the game runs.
            Scene::Character => (arena::training_arena(&ArenaConfig::with_group(0)), None),
            // The VFX test bed: a floor, three dummies to aim effects at and judge scale
            // against, and no environment in the way of looking at an explosion.
            Scene::Vfx => (
                arena::training_arena(&ArenaConfig::with_group(VFX_SCENE_DUMMIES)),
                None,
            ),
            // The audio test bed: the same floor and dummies the VFX bed uses. Sound is
            // judged by walking around it, so what the scene needs is room to walk and
            // something to stand next to for a sense of distance.
            Scene::Audio => (
                arena::training_arena(&ArenaConfig::with_group(AUDIO_SCENE_DUMMIES)),
                None,
            ),
            // The encounter test bed: the training arena's floor, with a fight standing on
            // it instead of a group of dummies. The same arena builder, because a scene is
            // gameplay data and a headless test has to be able to build the identical one.
            Scene::Encounter => {
                let config = ArenaConfig::with_group(0);
                let encounter = if args.enemies == 0 {
                    arena::demo_encounter(&config)
                } else {
                    arena::pack_encounter(&config, args.enemies)
                };
                log::info!(
                    "encounter '{}': {} enemies",
                    encounter.name(),
                    encounter.spawns().len()
                );
                (arena::encounter_arena(&config, encounter), None)
            }
            // The dungeon: five rooms, eight packs and a boss, built by gameplay from the
            // same scene the renderer is about to draw.
            Scene::Dungeon => {
                let (simulation, dungeon) = engine_game::dungeon::build_dungeon();
                log::info!(
                    "dungeon '{}': {} packs across {} areas",
                    dungeon.name(),
                    dungeon.packs().len(),
                    dungeon.areas().len(),
                );
                (simulation, None)
            }
            // The line-of-sight course: three domes, six dummies standing on two of them,
            // and a floor with two pits in it. Built by gameplay from the same scene the
            // renderer is about to draw.
            Scene::LineOfSight => {
                let (simulation, course) = engine_game::los_course::los_course_arena();
                for dummy in &course.dummies {
                    log::info!(
                        "line of sight: {} {} should be {}",
                        dummy.dome,
                        dummy.name,
                        dummy.expected.as_str()
                    );
                }
                (simulation, None)
            }
            // The shadow course: the scene's own floor and props, and nobody standing on
            // them but the player. One dummy would be a scale reference, as it is in the
            // environment bed — but a dummy is a *skinned* caster, and a skinned caster is
            // the one thing here that has to be judged animating rather than standing, so
            // it is judged in the combat scene where it moves.
            Scene::Shadows => (
                match environment.as_ref() {
                    Some(scene) => arena::environment_arena(scene, 0),
                    None => arena::training_arena(&ArenaConfig::with_group(0)),
                },
                None,
            ),
            Scene::Collision => {
                let (simulation, scene) = demo::collision_demo();
                (simulation, Some(scene))
            }
        };
        crate::startup::stage("simulation build");
        // The environment library, uploaded once: sixteen `create_model` calls, whatever the
        // scene's object count is. Every placement of an asset then shares the mesh and
        // material handles that upload returned, which is the whole of the instancing
        // story — see `engine_presentation::environment`.
        let mut placer = engine_presentation::EnvironmentPlacer::new(match environment {
            Some(_) => upload_environment(&mut renderer)?,
            None => engine_presentation::EnvironmentVisuals::new(),
        });

        crate::startup::stage("GPU upload: environment");

        let mut extractor = RenderExtractor::new(visuals, GROUND_HALF_EXTENT * 2.0);
        // The player's model, skeleton and clips. Every scene gets one, including the
        // collision course — a character test needs a character, and a capsule that walks is
        // easier to judge when it has legs.
        let heroes = HeroVisuals::upload(&mut renderer)?;
        extractor.set_player_visual(heroes.visual(simulation.hero())?);
        crate::startup::stage("GPU upload: heroes");
        if args.scene.is_collision() {
            // The demo course brings its own floor: a slab whose top face is at y = 0. The
            // extractor's floor plane is at y = 0 too, and two coplanar surfaces are a
            // depth-buffer tie — which reads as the whole floor flickering. Dropped to the
            // course's catch floor, where it is the thing a long fall lands on instead.
            extractor.set_ground_height(demo::VOID_Y);
        }
        if let Some(scene) = environment.as_ref() {
            // The scene brings its own ground: a plain out to the horizon and a paved arena
            // two centimetres above it. The extractor's placeholder plane is at y = 0 too,
            // and two coplanar surfaces are a depth-buffer tie that reads as the whole floor
            // flickering — so it is hidden rather than moved. Hidden and not despawned,
            // because the extractor owns that entity and would keep a stale handle.
            let ground = extractor.ground();
            extractor.render_world_mut().set_visible(ground, false);
            // Anything the level names that the library does not have: a `.glb` under
            // assets/, or the marker for a name nothing answers to.
            let extra = upload_missing_models(&mut renderer, placer.visuals_mut(), scene);
            crate::startup::stage("models from disk");
            if scene.terrain.is_some() {
                log::info!(
                    "environment '{}': terrain, {} points a side",
                    scene.name,
                    scene
                        .terrain
                        .as_ref()
                        .map_or(0, |terrain| terrain.resolution)
                );
            }
            if extra > 0 {
                log::info!(
                    "environment '{}': {extra} model(s) uploaded from disk",
                    scene.name
                );
            }
            placer.place(
                extractor.render_world_mut(),
                scene,
                controller_eye_guess(scene),
                LOD_BIAS,
            );
            let stats = placer.stats();
            if stats.unresolved > 0 {
                // A scene naming an asset the library does not have. Loud, because the
                // object is simply not drawn and an empty space is indistinguishable from
                // an object that was never meant to be there.
                log::error!(
                    "environment '{}': {} of {} objects name an asset that was not uploaded",
                    scene.name,
                    stats.unresolved,
                    stats.objects
                );
            }
            log::info!(
                "environment '{}': {} objects, {} render entities, {} models uploaded",
                scene.name,
                stats.objects,
                stats.entities,
                placer.visuals().len()
            );
        }
        crate::startup::stage("scene placement");
        extractor.extract(&simulation);

        // The collision course is only visible through the debug view, so in that scene the
        // view starts on: it *is* the scene's picture, not an overlay on top of one. F3
        // still turns it off, which is how a developer checks that turning it off works.
        #[cfg(feature = "debug-overlay")]
        let shadow_debug = ShadowCasterDebug::new(debug_visuals);
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
            "settings: {} (camera sensitivity {:.2}, interface size {:.2}, nameplate size {:.0},              health regen {:.1}%/s)",
            game_settings.path().display(),
            game_settings.settings().camera_sensitivity,
            game_settings.settings().ui_scale,
            game_settings.settings().nameplates.size,
            game_settings
                .settings()
                .gameplay
                .health_regen_percent_per_second
        );
        // The player's regeneration rate reaches gameplay here and nowhere else: the scene
        // built the simulation with the compiled-in default, and this is the one place that
        // knows the file said otherwise.
        simulation
            .set_health_regen_per_second(game_settings.settings().gameplay.health_regen_fraction());
        let ui_scale = UiScale::new(game_settings.settings().ui_scale);

        // The HUD layout, in its own file. A first launch has no file, and only then is bar
        // one filled in with the class's abilities and their keys: after that the file *is*
        // the arrangement, and a bar the player deliberately cleared must come back cleared.
        //
        // A launch is not a swap, and the difference used to be a bug. Bar one follows the
        // class, a swap rewrites it, and nothing rewrote it on the way back in: a player who
        // quit as the Druid came back as the FireMage with ten Druid spells on bar one,
        // which draw as empty slots because the FireMage cannot cast them.
        // `ActionBarSet::for_hero` is what closes that — it rebinds a foreign bar, or hands
        // back this character's own bars if the player keeps them per character.
        let binding_scope = game_settings.settings().binding_scope;
        let mut hud_config = HudConfigManager::load_from_game_directory();
        let action_bars = if hud_config.existed() {
            ActionBarSet::for_hero(hud_config.config(), binding_scope, simulation.hero())
        } else {
            ActionBarSet::with_default_bindings(simulation.hero())
        };
        // Written back whichever branch produced them: a first launch has no file to write,
        // and a launch that rebound a foreign bar has a file that no longer says what the
        // game is now showing.
        let mut fresh = hud_config.config().clone();
        action_bars.store(&mut fresh, binding_scope, simulation.hero());
        hud_config.update(fresh);
        hud_config.save_if_dirty();
        // What each character owns. A first launch has no file, and the roster keeps the
        // starting gear the simulation dressed it in; afterwards the file *is* what they
        // own, empty slots and all.
        let character_config = CharacterConfigManager::load_from_game_directory();
        if character_config.existed() {
            engine_presentation::character_save::apply_config(
                character_config.config(),
                simulation.roster_mut(),
            );
            simulation.refresh_character();
            log::info!("character: loaded {}", character_config.path().display());
        }

        let hud_layout = HudLayout::new(hud_config.config().clone());
        log::info!(
            "hud: {} ({} bars enabled)",
            hud_config.path().display(),
            (0..engine_settings::hud::ACTION_BARS)
                .filter(|index| hud_layout.is_enabled(HudElementId::ActionBar(*index)))
                .count()
        );

        crate::startup::stage("settings and HUD");

        let mut controller = ThirdPersonCamera::following(simulation.player_position());
        controller.set_sensitivity(game_settings.settings().camera_sensitivity);
        let mut camera = Camera::new(Projection::perspective(aspect_of(width, height)))?;
        controller.apply(&mut camera, aspect_of(width, height))?;

        // The scene's light, where there is a scene. Lighting is content — a boss arena and
        // a training yard want different suns — so it lives beside the objects it lights and
        // reaches the renderer through a five-field copy.
        //
        // The fallback is the pre-Phase-2 sun, kept for the two scenes that bring no
        // environment: a collision course lit for composition rather than for reading
        // geometry is a course that is harder to test in.
        let (lighting, clear_color) = match environment.as_ref() {
            Some(scene) => (
                engine_presentation::scene_lighting(&scene.lighting),
                scene.lighting.clear_color(),
            ),
            None => (
                Lighting {
                    direction: Vec3::new(-0.45, -0.85, -0.35),
                    color: Vec3::new(1.0, 0.97, 0.90),
                    intensity: 1.15,
                    ambient_color: Vec3::new(0.55, 0.62, 0.80),
                    ambient_intensity: 0.28,
                },
                CLEAR_COLOR,
            ),
        };

        // The device, and the runtime it will mix. Opened before the first frame so the
        // synthesis cost — a few million samples of arithmetic — is paid at load rather than
        // on the frame the first spell is cast.
        //
        // `--no-audio` builds the same runtime with no device and no samples, which is the
        // control the performance audit compares against: cues are still made, limited and
        // counted, so the difference between the two runs is the mix and the hardware.
        let audio = if args.audio {
            // Opened on a thread of its own: enumerating the device and synthesising the
            // bank is about 120 ms, and the first frame needs none of it. The runtime is
            // real from this instant — it simply has no samples yet, which is the same
            // thing a machine with no sound card has for the whole session.
            AudioHost::open_in_background()
        } else {
            log::info!("audio: --no-audio; no device will be opened");
            AudioHost::silent(crate::audio_device::FALLBACK_SAMPLE_RATE)
        };
        crate::startup::stage("audio init");
        let audio_director = AudioDirector::new();
        apply_volumes(&audio, game_settings.settings());
        let settings_now = game_settings.settings();
        log::info!(
            "audio: {} | master {:.2}, sfx {:.2}, ui {:.2}, ambient {:.2}",
            if args.audio {
                "opening in the background"
            } else {
                "silent"
            },
            settings_now.master_volume,
            settings_now.sfx_volume,
            settings_now.ui_volume,
            settings_now.ambient_volume,
        );

        // Built before the first frame so the UI never sees a default-constructed snapshot.
        let snapshot = UiSnapshot::build(&simulation);

        let report_interval = settings
            .reports_enabled()
            .then(|| Duration::from_secs_f64(settings.report_interval_seconds));

        log_controls(args, demo.as_ref());

        // The shadow quality the settings file asked for, applied before the first frame:
        // the renderer starts at its own default, and a player who chose "off" should never
        // see a frame that was not.
        // `--shadows` overrides the file for this run without writing to it: a measurement
        // or a bug report should not change what the player sees next time they start.
        let shadow_quality = args
            .shadows
            .unwrap_or(game_settings.settings().graphics.shadows);

        crate::startup::stage("app assembly");
        let mut app = Self {
            window,
            renderer,
            simulation,
            extractor,
            controller,
            camera,
            lighting,
            clear_color,
            environment,
            placer,
            terrain_entity: None,
            water_entities: Vec::new(),
            #[cfg(feature = "editor")]
            spawn_marker_entities: Vec::new(),
            bindings: Bindings::default(),
            scene: args.scene,
            #[cfg(feature = "editor")]
            editor: engine_editor::Editor::new(),
            #[cfg(feature = "editor")]
            editor_open_requested: args.editor,
            #[cfg(feature = "editor")]
            editor_save_path: args.scene_file.as_deref().map(std::path::PathBuf::from),
            demo,
            collision_ui: UiDrawList::with_capacity(64, 1_024),
            #[cfg(feature = "debug-overlay")]
            collision_debug,
            shadow_debug_view: 0,
            #[cfg(feature = "debug-overlay")]
            shadow_debug,
            commands: Vec::with_capacity(16),
            heroes,
            menu: MenuSystem::new(game_settings.settings(), env!("CARGO_PKG_VERSION")),
            menu_actions: Vec::with_capacity(4),
            menu_theme: MenuTheme::default(),
            menu_scratch: MenuScratch::new(),
            settings: game_settings,
            hud_layout,
            hud_config,
            action_bars,
            hud_editor: HudEditor::new(),
            slot_menu: SlotMenu::new(),
            frame_scratch: FrameScratch::new(),
            hud_scratch: String::with_capacity(48),
            targeting: GroundTargeting::new(),
            talent_menu: TalentMenu::with_scale(ui_scale),
            talent_theme: TalentTheme::default(),
            talent_scratch: String::with_capacity(32),
            character_menu: CharacterMenu::with_scale(ui_scale),
            character_theme: CharacterTheme::default(),
            character_scratch: String::with_capacity(64),
            character_config,
            hovered_ability: None,
            hovered_slot: None,
            targeting_decal: None,
            decal_scratch: engine_presentation::ui::ground_decal::DecalScratch::new(),
            decal_border: Vec::new(),
            decal_glow: Vec::new(),
            decal_visuals,
            affected: Vec::with_capacity(32),
            mouse_look: MouseLook::new(),
            motion: RelativeMotion::new(),
            cursor_before_look: (0, 0),
            click_candidate: None,
            click_travel: 0,
            extract_ms: 0.0,
            vfx_ms: 0.0,
            ui_ms: 0.0,
            nameplates: NameplateSet::with_capacity(32),
            frame_seconds: 0.0,
            snapshot,
            game_ui: GameUi::with_scale(ui_scale),
            numbers: FloatingNumbers::new(),
            vfx: VfxDirector::new(vfx_visuals),
            audio,
            audio_director,
            audio_ms: 0.0,
            audio_demo_selection: 0,
            vfx_stress: args.vfx_stress.max(0.0),
            vfx_stress_debt: 0.0,
            vfx_stress_next: 0,
            #[cfg(feature = "debug-overlay")]
            last_effect: None,
            report_interval,
            show_window: args.window,
            window_shown: false,
            warmup_frames: args.warmup,
            shadow_overrides: args.shadow_overrides,
            measured_from: None,
            benchmark: args.benchmark.as_ref().map(|resolutions| {
                log::info!(
                    "benchmark: {} resolution(s) x {} pass(es), {} warm-up + {} measured \
                     frame(s) each, in one process",
                    resolutions.len(),
                    args.benchmark_passes,
                    args.benchmark_warmup,
                    args.benchmark_frames
                );
                ResolutionBenchmark::new(
                    resolutions,
                    args.benchmark_passes,
                    args.benchmark_warmup,
                    args.benchmark_frames,
                )
            }),
            shadow_gpu: engine_core::time::FrameStatsWindow::new(1024),
            last_report: Instant::now(),
            report: String::new(),
            #[cfg(feature = "debug-overlay")]
            overlay_lines: Vec::with_capacity(32),
            #[cfg(feature = "debug-overlay")]
            last_overlay_update: Instant::now(),
        };
        // The ground, if the level has a shape for it. Built here rather than in the loop
        // above because it is one mesh for the whole level and it needs the app's own
        // renderer and world — the same call a brush stroke makes, so a loaded terrain and an
        // edited one cannot end up drawn by two different paths.
        app.rebuild_terrain();
        app.rebuild_water();
        // Everything since `begin_upload_batch` queued its copies; this is the one wait. The
        // ground and the water are inside it deliberately — they are a mesh each, and a mesh
        // that waits for the GPU on its own is the cost this batch exists to avoid.
        // Nothing has been drawn yet, so nothing has read a resource that was still in
        // flight.
        let copies = app.renderer.end_upload_batch()?;
        log::info!("startup: {copies} resource copies in one submission");
        crate::startup::stage("GPU flush");

        let applied = app.apply_shadow_quality(shadow_quality);
        // The settings screen shows what is running, which after a failed switch is not
        // necessarily what the file asked for.
        if applied {
            app.menu.set_shadow_quality(shadow_quality);
        }
        if let Some(enabled) = args.shadow_overrides.clusters {
            app.renderer.set_light_clustering(enabled);
            log::info!("light clustering: {}", if enabled { "on" } else { "off" });
        }
        if let Some(enabled) = args.shadow_overrides.local_lights {
            app.renderer.set_local_lights_enabled(enabled);
            log::info!("local lights: {}", if enabled { "on" } else { "off" });
        }
        Ok(app)
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
            // Audio off and on, as a *developer* switch rather than a mute: it stops the
            // work, not just the sound, which is what makes "with audio" and "without audio"
            // two measurable states of one build.
            // Interrupts the target's cast. A *developer* control: the class has no
            // interrupt ability, and inventing one would be a new player ability this phase
            // is explicitly not adding. The mechanism it drives is the shipping one.
            // H rather than G: G is the second of the two ability buttons beside the bar
            // (see `Bindings::extra_two`), and a developer control has no business taking a
            // key the class needs.
            KeyCode::H => {
                let Some(target) = self.simulation.player().target else {
                    return;
                };
                if self.simulation.interrupt(target) {
                    log::info!("interrupted {}", target.index());
                } else {
                    log::info!("nothing to interrupt on {}", target.index());
                }
            }
            KILL_PLAYER_KEY => {
                // Nothing is queued and nothing is bypassed: killing an already-dead player
                // does nothing, so a held key cannot stack up deaths or corrupt the clock.
                if self.simulation.player_alive() {
                    self.simulation.kill_player();
                    log::info!("F7: player killed");
                } else {
                    log::info!("F7: the player is already dead");
                }
            }
            KeyCode::F8 => {
                let enabled = !self.audio_director.enabled();
                if let Ok(mut audio) = self.audio.system().lock() {
                    self.audio_director.set_enabled(enabled, &mut audio);
                    audio.set_enabled(enabled);
                }
                log::info!("audio {}", if enabled { "on" } else { "off" });
            }
            KeyCode::F9 => {
                let enabled = !self.renderer.wireframe();
                self.renderer.set_wireframe(enabled);
                log::info!("wireframe {}", if enabled { "on" } else { "off" });
            }
            SHADOW_QUALITY_KEY => self.cycle_shadow_quality(),
            SHADOW_DEBUG_KEY => {
                // Nine stops, cycled. Stops 1 to 3 are shadow views the renderer's pixel
                // shader owns; stop 4 draws the caster bounds, which is presentation's
                // geometry rather than the renderer's, so it is tracked as "mode 4 means
                // renderer mode 0 with the bounds on" instead of being a branch in the pixel
                // shader. Stops 5 to 8 are the material views, which the shader numbers 8 to
                // 11 - what the lighting model made of the material set at this pixel, which
                // is the only way to look at a normal map or a roughness map on real content
                // rather than on a test quad.
                let previous = self.shadow_debug_view;
                let mode = (previous + 1) % 9;
                self.shadow_debug_view = mode;
                self.renderer.set_shadow_debug_mode(match mode {
                    4 => 0,
                    5..=8 => mode + 3,
                    other => other,
                });
                #[cfg(feature = "debug-overlay")]
                self.shadow_debug
                    .set_enabled(self.extractor.render_world_mut(), mode == 4);
                log::info!(
                    "debug view: {}",
                    match mode {
                        1 => "cascade tint",
                        2 => "shadow factor",
                        3 => "the atlas, bottom left",
                        4 => "caster bounds",
                        5 => "shading normal, after the normal map",
                        6 => "roughness",
                        7 => "metallic",
                        8 => "ambient occlusion",
                        _ => "off",
                    }
                );
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

    /// Moves to the next shadow quality level and applies it.
    ///
    /// The setting is written back through the settings manager, so the level survives a
    /// restart — a quality key that only changed the frame would be a key that lies about
    /// what it did.
    fn cycle_shadow_quality(&mut self) {
        let next = self.settings.settings().graphics.shadows.next();
        // Through the same path the settings screen uses, so the key and the screen cannot
        // end up disagreeing about which level is running.
        self.choose_shadow_quality(next);
    }

    /// Tells the renderer what a shadow quality level means.
    ///
    /// The conversion from the player's word to the renderer's numbers, done here because
    /// the application is the one place that knows both `engine-settings` and
    /// `engine-renderer` — the same rule the scene's lighting follows.
    fn apply_shadow_quality(&mut self, quality: engine_settings::ShadowQuality) -> bool {
        let preset = match quality {
            engine_settings::ShadowQuality::Off => ShadowPreset::Off,
            engine_settings::ShadowQuality::Low => ShadowPreset::Low,
            engine_settings::ShadowQuality::Medium => ShadowPreset::Medium,
            engine_settings::ShadowQuality::High => ShadowPreset::High,
            engine_settings::ShadowQuality::Ultra => ShadowPreset::Ultra,
        };
        // `--shadow-tweak` on top of the preset: a benchmark moves one field without
        // inventing a quality level for it, and F4 still cycles the presets underneath.
        let settings = self.shadow_overrides.apply(preset.settings());
        if let Err(error) = self.renderer.set_shadow_settings(settings) {
            // A quality change that fails leaves the previous configuration running, which
            // is a worse picture rather than a broken one.
            log::error!("shadows: could not switch to {}: {error}", quality.as_str());
            return false;
        }
        let (width, height) = settings.atlas_size();
        log::info!(
            "shadows: {} | {} cascade(s) at {} | atlas {width}x{height} ({:.1} MiB) | \
             pcf {} | local views {} | contact {}{}",
            quality.as_str(),
            settings.cascade_count,
            settings.resolution,
            settings.atlas_bytes() as f64 / (1024.0 * 1024.0),
            settings.pcf.as_str(),
            settings.local_view_budget(),
            if settings.contact.enabled {
                "on"
            } else {
                "off"
            },
            if settings.contact.enabled {
                format!(
                    ", {} step(s) at 1/{}",
                    settings.contact.steps, settings.contact.divisor
                )
            } else {
                String::new()
            },
        );
        true
    }

    /// Applies a shadow level the player chose, and writes it to disk.
    ///
    /// The menu's tick is moved **here**, after the renderer has accepted the level, and not
    /// by the click that asked for it: switching quality recreates the shadow atlas behind a
    /// `wait_for_idle` and can fail, and a screen that had already ticked the new row would
    /// be showing a level that is not running. On failure the setting goes back to what it
    /// was, so the file and the frame never disagree either.
    fn choose_shadow_quality(&mut self, quality: engine_settings::ShadowQuality) {
        let previous = self.settings.settings().graphics.shadows;
        if quality == previous {
            // Still worth re-ticking: the click moved the checkbox the player pressed, and
            // this is what puts it back.
            self.menu.set_shadow_quality(previous);
            return;
        }

        if self.apply_shadow_quality(quality) {
            let mut settings = self.settings.settings();
            settings.graphics.shadows = quality;
            self.settings.update(settings);
            if self.settings.save_if_dirty() {
                log::info!("settings: shadows are now {}", quality.as_str());
            }
            self.menu.set_shadow_quality(quality);
        } else {
            // The renderer refused; the previous configuration is still the one running.
            self.menu.set_shadow_quality(previous);
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
        // The press is not a click any more either. Something else took the pointer — a spell
        // being aimed, a menu opening — and the button coming up afterwards belongs to that,
        // not to selecting whatever the cursor was over before it appeared.
        self.click_candidate = None;
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
        let scale = self.game_ui.scale();
        let ui_input = UiInput::from_input(ctx.input());
        self.snapshot
            .refresh(&self.simulation, self.aiming_ability());

        // Edit HUD, which replaces the interface's ordinary behaviour entirely. Nothing
        // below runs while it is on, which is what makes "a click that moves a bar cannot
        // also cast what is under the cursor" a property of the control flow rather than a
        // rule every handler has to remember.
        if self.hud_editor.is_active() {
            self.hovered_ability = None;
            self.hovered_slot = None;
            match self
                .hud_editor
                .update(&ui_input, &mut self.hud_layout, scale, viewport)
            {
                EditOutcome::Changed => self.save_hud_layout(),
                EditOutcome::Closed => {
                    self.save_hud_layout();
                    log::info!("hud: edit mode closed");
                }
                EditOutcome::Idle => {}
            }
            return;
        }

        // The slot menu, which covers the bar under it.
        if self.slot_menu.is_open() {
            self.hovered_ability = None;
            self.hovered_slot = None;
            let input = ui_input.with_captured_key(ctx.input());
            let outcome = self
                .slot_menu
                .update(&input, &self.action_bars, &self.bindings);
            self.apply_slot_menu(outcome);
            return;
        }

        // The character key, read here rather than in the input map for the reason the bar's
        // keys are: it toggles a window, and whether a press opens or closes one is a
        // question about the interface rather than about the character. Read after the slot
        // menu so a rebinding capture that is waiting for a key gets it first.
        if ctx.input().was_key_pressed(self.bindings.character_sheet) {
            if self.character_menu.is_open() {
                self.character_menu.close();
            } else {
                self.open_character_sheet();
            }
            return;
        }

        // The character sheet, which covers everything while it is up. Above the talent
        // menu in this chain only because two windows cannot both be open: opening either
        // closes the other, so the order decides nothing beyond which is asked first.
        if self.character_menu.is_open() {
            self.hovered_ability = None;
            self.hovered_slot = None;
            match self
                .character_menu
                .update(&ui_input, &self.snapshot, viewport)
            {
                CharacterMenuOutcome::Move { from, to } => {
                    self.simulation
                        .push_command(PlayerCommand::MoveItem { from, to });
                    // Written on the drop rather than on the way out: a crash between
                    // equipping something and closing the panel should not cost the
                    // player the item they just put on.
                    self.save_characters();
                }
                CharacterMenuOutcome::Idle | CharacterMenuOutcome::Closed => {}
            }
            return;
        }

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

        // The two corner buttons.
        let talent_button = self.game_ui.talent_button_rect(viewport);
        if ui_input.pointer_pressed
            && talent_button.contains(ui_input.pointer.0, ui_input.pointer.1)
        {
            self.open_talents();
            return;
        }
        let character_button = self.game_ui.character_button_rect(viewport);
        if ui_input.pointer_pressed
            && character_button.contains(ui_input.pointer.0, ui_input.pointer.1)
        {
            self.open_character_sheet();
            return;
        }

        // Placing a ground-targeted ability.
        if self.targeting.is_active() {
            let caster = self.simulation.player_position();
            let view_projection = self.camera.view_projection();
            // The level's own surface, so the cursor lands on the terrain the player is
            // looking at rather than on the plane the arena used to be.
            let terrain = self
                .environment
                .as_ref()
                .and_then(|scene| scene.terrain.as_ref());
            let ground = match terrain {
                Some(terrain) => GroundSurface::Terrain(terrain),
                None => GroundSurface::Plane(engine_presentation::ui::targeting::GROUND_HEIGHT),
            };
            match self
                .targeting
                .update(&ui_input, &view_projection, viewport, caster, &ground)
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

        // The bars: what the pointer is over, what it clicked, and what it right-clicked.
        self.hovered_ability = None;
        self.hovered_slot = None;
        let mut pressed = None;
        let mut open_menu = None;
        for index in 0..engine_settings::hud::ACTION_BARS {
            let id = HudElementId::ActionBar(index);
            // A disabled bar is skipped here as well as in the painter, so it takes up no
            // interaction space at all rather than being an invisible thing to click on.
            if !self.hud_layout.is_enabled(id) {
                continue;
            }
            let rect = self.hud_layout.rect_of(id, scale, viewport);
            let orientation = self.hud_layout.orientation(index);
            let Some(slot_index) =
                action_bar::slot_at(rect, orientation, ui_input.pointer.0, ui_input.pointer.1)
            else {
                continue;
            };
            let address = SlotAddress::new(index, slot_index);
            let Some(slot) = self.action_bars.slot(address) else {
                continue;
            };
            if ui_input.secondary_pressed {
                open_menu = action_bar::slot_rect(rect, orientation, slot_index)
                    .map(|cell| (address, cell));
            }
            let Some(ability) = slot.ability else {
                continue;
            };
            self.hovered_ability = Some(ability);
            self.hovered_slot = Some(address);
            if ui_input.pointer_pressed {
                pressed = Some(ability);
            }
        }
        if let Some((address, cell)) = open_menu {
            // Right click assigns rather than casts, which is the genre's convention and the
            // one gesture the bar was not already using.
            self.cancel_mouse_look();
            self.slot_menu.set_scale(scale);
            self.slot_menu.open(address, cell, viewport);
            self.play_ui_sound(engine_audio::SoundId::UiClick);
            return;
        }

        // Only for a hero that has one. The Warrior and the Druid draw no Ultimate button,
        // and a rectangle that answers the pointer where nothing is drawn is a dead zone the
        // player cannot see.
        let ultimate = self.game_ui.ultimate_rect(viewport);
        if let Some(view) = self.snapshot.ultimate
            && ultimate.contains(ui_input.pointer.0, ui_input.pointer.1)
        {
            self.hovered_ability = Some(view.ability);
            self.hovered_slot = None;
            if ui_input.pointer_pressed {
                pressed = Some(view.ability);
            }
        }

        // The extras beside it, hit-tested at the rectangles they are drawn at.
        for index in 0..self.snapshot.extras.len() {
            let Some(view) = self.snapshot.extras[index] else {
                continue;
            };
            let rect = self.game_ui.extra_rect(index, viewport);
            if rect.contains(ui_input.pointer.0, ui_input.pointer.1) {
                self.hovered_ability = Some(view.ability);
                self.hovered_slot = None;
                if ui_input.pointer_pressed {
                    pressed = Some(view.ability);
                }
            }
        }

        // The keys. Every slot's key is read here rather than in the input map: the bar is
        // the binding table now, and an aimed ability's key opens its targeting circle
        // instead of casting, which is a decision only this layer can make.
        for (_, slot) in self.action_bars.occupied() {
            let (Some(key), Some(ability)) = (slot.key, slot.ability) else {
                continue;
            };
            if ctx.input().was_chord_pressed(key) {
                pressed = Some(ability);
            }
        }
        // Whichever ability *this* hero keeps on the Ultimate key, rather than the mage's:
        // the Warrior's Last Stand and the Druid's Tranquility live in the same slot, and a
        // key that always meant Cataclysm was a key two of the three heroes could not press.
        if ctx.input().was_key_pressed(self.bindings.ultimate)
            && let Some(ultimate) = self.simulation.hero().ultimate()
        {
            pressed = Some(ultimate);
        }
        // And the two buttons beside it, for a hero with more abilities than digits.
        for (index, ability) in self.simulation.hero().extras().enumerate() {
            let Some(key) = self.bindings.extra_key(index) else {
                continue;
            };
            if ctx.input().was_key_pressed(key) {
                pressed = Some(ability);
            }
        }

        if let Some(ability) = pressed {
            self.cast_or_target(ability);
        }
    }

    /// The rectangle of the slot the pointer is over, if it is over one.
    ///
    /// The slot is the one the pointer was over, remembered when it was found; only its
    /// rectangle is worked out again, from the same layout the click was tested against, so
    /// the two cannot disagree about where a slot is after a resize between the two calls.
    fn hovered_slot_rect(&self, viewport: (u32, u32)) -> Option<UiRect> {
        let address = self.hovered_slot?;
        let id = HudElementId::ActionBar(address.bar);
        if !self.hud_layout.is_enabled(id) {
            return None;
        }
        let scale = self.game_ui.scale();
        let bar = self.hud_layout.rect_of(id, scale, viewport);
        let orientation = self.hud_layout.orientation(address.bar);
        action_bar::slot_rect(bar, orientation, address.slot)
    }

    /// Carries out what the slot menu asked for, and writes the result to disk.
    ///
    /// The menu changes nothing itself; this is the only place a slot's spell or key moves,
    /// which is what keeps the assignment, the conflict rule and the save in one place.
    fn apply_slot_menu(&mut self, outcome: SlotMenuOutcome) {
        let changed = match outcome {
            SlotMenuOutcome::Idle => return,
            SlotMenuOutcome::Closed => false,
            SlotMenuOutcome::AssignSpell(address, ability) => {
                self.action_bars.assign_spell(address, ability)
            }
            SlotMenuOutcome::AssignKey(address, key) => {
                self.action_bars.assign_key(address, key, &self.bindings)
            }
            SlotMenuOutcome::ClearSlot(address) => self.action_bars.clear_slot(address),
        };
        if changed {
            self.save_hud_layout();
        }
    }

    /// Writes the arrangement and the bars to `hud.cfg`, if either changed.
    ///
    /// Both together, because they share one file and one validation pass: the layout owns
    /// where a bar is, the bars own what is in it, and the file is the two merged.
    /// Opens the character sheet, closing whatever other window was up.
    ///
    /// Two full-screen windows open at once would each eat the other's clicks, so opening
    /// one closes the other here rather than in either window's own update.
    fn open_character_sheet(&mut self) {
        self.talent_menu.close();
        self.hovered_ability = None;
        self.hovered_slot = None;
        self.character_menu.open();
    }

    /// Opens the talent window, closing the character sheet.
    fn open_talents(&mut self) {
        self.character_menu.close();
        self.talent_menu.open();
    }

    /// Writes what every character is wearing and carrying.
    ///
    /// The roster is the authority; the file is a copy of it. Nothing reads the panel to
    /// decide what was saved, which is what keeps a half-finished drag out of the save.
    fn save_characters(&mut self) {
        let config = engine_presentation::character_save::config_of(self.simulation.roster());
        self.character_config.update(config);
        if self.character_config.save_if_dirty() {
            log::debug!("character: saved");
        }
    }

    fn save_hud_layout(&mut self) {
        let mut config = self.hud_layout.config().clone();
        // The layout owns where everything sits and knows nothing about characters, so the
        // per-character records are carried across from the file rather than dropped by a
        // copy that never had them.
        config.heroes.clone_from(&self.hud_config.config().heroes);
        self.action_bars
            .store(&mut config, self.binding_scope(), self.simulation.hero());
        self.hud_config.update(config);
        if self.hud_config.save_if_dirty() {
            log::debug!("hud: layout saved");
        }
    }

    /// The ability whose area is being placed, if the player is placing one.
    ///
    /// The one thing on the ability bar the simulation cannot answer: an unconfirmed
    /// placement has changed nothing, so it lives here beside the targeting circle it
    /// belongs to and reaches the interface through the snapshot like everything else.
    fn aiming_ability(&self) -> Option<FireAbility> {
        self.targeting.active().map(|target| target.ability)
    }

    /// Becomes `hero`: the simulation, the model, the bar and the roster's tick, in that
    /// order.
    ///
    /// ```text
    /// Simulation::set_hero   the abilities, and a clean class state
    ///          │
    ///          ├── HeroVisuals::visual      the model the player is drawn as
    ///          ├── ActionBarSet::rebind     bar one, saved to the HUD file
    ///          └── MenuSystem::set_hero     the tick, reported back from the game
    /// ```
    ///
    /// The world is untouched. The player keeps their position, their health, their target
    /// and the fight they are in: a hero swap is a change of what you are holding, not a
    /// reload, and treating it as one would make it useless as the thing it is for — trying
    /// the three classes against the same dummies.
    ///
    /// Anything the old hero had open goes with it. A Meteor being placed by a mage who is
    /// now a Warrior has no ability behind it, and a targeting circle for a spell nobody can
    /// cast is the one state this must not leave behind.
    fn swap_hero(&mut self, hero: Hero) {
        // Read before the simulation is told: per character, the bars being replaced are the
        // ones this character had, and after `set_hero` there is nothing left to ask.
        let previous = self.simulation.hero();
        if !self.simulation.set_hero(hero) {
            // Already this hero. The tick is still refreshed, because the menu may have
            // moved it optimistically.
            self.menu.set_hero(hero);
            return;
        }
        self.targeting.cancel();
        // The picker offers the hero's own spells, so it is told before it is next opened.
        self.slot_menu.set_hero(hero);
        self.hovered_ability = None;
        self.hovered_slot = None;

        match self.heroes.visual(hero) {
            Ok(visual) => self.extractor.set_player_visual(visual),
            // The model failed to bind, which upload has already ruled out. The character
            // keeps the model it had rather than vanishing, and the line says which hero has
            // the wrong body.
            Err(error) => log::error!("hero swap: {} kept the old model: {error}", hero.name()),
        }

        // What the swap does to the bars is the player's setting, not a rule:
        //
        //   account    bar one follows the class; the other seven are the player's own
        //   character  the old character's bars are saved and the new character's loaded
        //
        // Either way it is written straight away, so the hero a player last chose is the bar
        // they come back to.
        let scope = self.binding_scope();
        let mut config = self.hud_config.config().clone();
        if scope.is_per_character() {
            self.action_bars.store(&mut config, scope, previous);
            self.action_bars = ActionBarSet::for_hero(&config, scope, hero);
        } else {
            self.action_bars.rebind_to_hero(hero);
        }
        self.action_bars.store(&mut config, scope, hero);
        self.hud_config.update(config);
        self.hud_config.save_if_dirty();

        self.menu.set_hero(hero);
        log::info!(
            "hero: now the {} ({} abilities)",
            hero.name(),
            hero.abilities().len()
        );
    }

    /// Casts an ability, or opens the targeting indicator if it is aimed with the mouse.
    ///
    /// An ability with no charge left, or one the global cooldown is still holding, is not
    /// selectable at all: opening a targeting circle for something that would be refused on
    /// the click wastes the player's attention and hides the reason.
    fn cast_or_target(&mut self, ability: FireAbility) {
        let mage = self.simulation.mage();
        if !mage.abilities.is_ready(ability) || !mage.off_global_cooldown() {
            return;
        }
        if ability.definition().energy_cost > 0.0 && !mage.energy.is_full() {
            return;
        }
        if ability.definition().target.is_aimed() {
            // Mouse-look would fight the indicator for the cursor, so it ends here.
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
            // Edit mode and the slot menu answer escape themselves, in the interface update
            // that runs after this one. Backing out of one of them must not also open the
            // pause menu on the same press.
            if self.hud_editor.is_active() || self.slot_menu.is_open() {
                return;
            }
            if self.character_menu.is_open() {
                self.character_menu.close();
                return;
            }
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
            // Then the selection, which is the last thing the player is "in" before they are
            // in nothing. Pressing escape twice from a fight therefore drops the target and
            // then opens the menu, which is the order every game in the genre uses.
            if self.simulation.player().target.is_some() {
                self.simulation.push_command(PlayerCommand::ClearTarget);
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
                MenuAction::Closed => {
                    log::info!("menu closed");
                }
                MenuAction::OpenedSettings => {
                    self.play_ui_sound(engine_audio::SoundId::UiClick);
                    // The screen shows what is in the settings, not what it last showed.
                    self.menu.apply_settings(self.settings.settings());
                }
                MenuAction::ClosedSettings => {
                    self.play_ui_sound(engine_audio::SoundId::UiCancel);
                }
                MenuAction::OpenedHeroes => {
                    self.play_ui_sound(engine_audio::SoundId::UiClick);
                    // The roster shows who the player *is*, which is the simulation's answer
                    // and not the menu's memory of it.
                    let hero = self.simulation.hero();
                    self.menu.set_hero(hero);
                }
                MenuAction::ClosedHeroes => {
                    self.play_ui_sound(engine_audio::SoundId::UiCancel);
                }
                MenuAction::HeroSelected(hero) => {
                    self.play_ui_sound(engine_audio::SoundId::UiSelect);
                    self.swap_hero(hero);
                }
                MenuAction::EditHudRequested => {
                    self.play_ui_sound(engine_audio::SoundId::UiClick);
                    // Mouse-look would fight the editor for the cursor, and a slot menu left
                    // open over the HUD would be a panel the editor could not move.
                    self.cancel_mouse_look();
                    self.slot_menu.close();
                    self.hud_editor.open();
                    log::info!("hud: edit mode");
                }
                MenuAction::QuitRequested => {
                    log::info!("quit requested from the menu");
                    // Through the engine's own lifecycle: the loop finishes this frame,
                    // `shutdown` runs, the renderer drains its frames in flight and the
                    // window closes. Nothing here terminates the process itself.
                    ctx.request_exit();
                }
                MenuAction::SensitivityChanged(value) => self.apply_sensitivity(value, false),
                MenuAction::SensitivityCommitted(value) => self.apply_sensitivity(value, true),
                MenuAction::UiScaleChanged(value) => self.apply_ui_scale(value, false),
                MenuAction::UiScaleCommitted(value) => self.apply_ui_scale(value, true),
                MenuAction::VolumeChanged(channel, value) => {
                    self.apply_volume(channel, value, false);
                }
                MenuAction::VolumeCommitted(channel, value) => {
                    self.apply_volume(channel, value, true);
                }
                MenuAction::BindingScopeCommitted(scope) => self.apply_binding_scope(scope),
                MenuAction::OpenedNameplates => {
                    self.play_ui_sound(engine_audio::SoundId::UiClick);
                    // The screen shows what is in the settings, not what it last showed.
                    self.menu.apply_settings(self.settings.settings());
                }
                MenuAction::ClosedNameplates => {
                    self.play_ui_sound(engine_audio::SoundId::UiCancel);
                }
                MenuAction::NameplateChanged(setting, value) => {
                    self.apply_nameplate(setting, value, false);
                }
                MenuAction::NameplateCommitted(setting, value) => {
                    self.apply_nameplate(setting, value, true);
                }
                MenuAction::OpenedGraphics => {
                    self.play_ui_sound(engine_audio::SoundId::UiClick);
                    // The screen shows what is in the settings, not what it last showed.
                    self.menu.apply_settings(self.settings.settings());
                }
                MenuAction::ClosedGraphics => {
                    self.play_ui_sound(engine_audio::SoundId::UiCancel);
                }
                MenuAction::ShadowQualityCommitted(quality) => {
                    self.play_ui_sound(engine_audio::SoundId::UiClick);
                    self.choose_shadow_quality(quality);
                }
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

    /// Applies an interface size, and writes it to disk if the interaction has finished.
    ///
    /// The value goes to the settings first and reaches the interface from there, so what is
    /// drawn is always what would be written to the file, snapping included.
    ///
    /// Every screen is told, not only the HUD: a menu laid out at one scale and a HUD at
    /// another would disagree about where things are, and the menu is itself the screen the
    /// slider lives on — so the player watches the change happen to the thing they are
    /// dragging. Nothing is rebuilt beyond the measurements, so this is a few dozen
    /// multiplies rather than anything that could be felt.
    fn apply_ui_scale(&mut self, value: f32, persist: bool) {
        self.settings.set_ui_scale(value);
        let scale = UiScale::new(self.settings.settings().ui_scale);
        self.game_ui.set_scale(scale);
        self.menu.set_scale(scale);
        self.talent_menu.set_scale(scale);
        self.character_menu.set_scale(scale);
        self.slot_menu.set_scale(scale);
        self.hud_editor.layout(scale, self.renderer.size());
        if persist && self.settings.save_if_dirty() {
            log::info!(
                "settings: interface size {:.2} saved",
                self.settings.settings().ui_scale
            );
        }
    }

    /// Where the action bars' spells and keys are kept.
    fn binding_scope(&self) -> BindingScope {
        self.settings.settings().binding_scope
    }

    /// Moves the bars to where the player has just said they should be kept.
    ///
    /// Turning per-character bars *on* saves what is on screen as this character's, so the
    /// arrangement the player is looking at is the one that character keeps rather than one
    /// they have to make again. Turning it *off* leaves every character's record in the file
    /// untouched: the setting can be turned back on, and nothing was thrown away by trying
    /// it. The bars on screen do not change in either direction — a setting about where
    /// something is stored must not rearrange the interface under the player's hands.
    fn apply_binding_scope(&mut self, scope: BindingScope) {
        if scope == self.binding_scope() {
            return;
        }
        self.settings.set_binding_scope(scope);
        self.save_hud_layout();
        if self.settings.save_if_dirty() {
            log::info!(
                "settings: action bars are now kept {}",
                if scope.is_per_character() {
                    "per character"
                } else {
                    "for the account"
                }
            );
        }
    }

    /// Applies a nameplate setting, and writes it to disk if the interaction has finished.
    ///
    /// Nothing else has to happen: the plates are rebuilt from the settings every frame, so
    /// the next one already uses the new value. That is the same property the layout depends
    /// on — a nameplate is recomputed from the world rather than stored anywhere — and it is
    /// why dragging one of these sliders shows the effect while the pointer is still down.
    fn apply_nameplate(&mut self, setting: NameplateSetting, value: f32, persist: bool) {
        self.settings.set_nameplate(setting, value);
        if persist && self.settings.save_if_dirty() {
            log::info!(
                "settings: nameplate {setting:?} {:.2} saved",
                self.settings.settings().nameplates.get(setting)
            );
        }
    }

    /// Plays one interface sound.
    ///
    /// The interface is the one thing that makes a sound without the simulation announcing
    /// anything, because pressing a button is not something that happens in the world.
    fn play_ui_sound(&mut self, sound: engine_audio::SoundId) {
        if let Ok(mut audio) = self.audio.system().lock() {
            self.audio_director.play_ui(sound, &mut audio);
        }
    }

    /// Applies a volume, and writes it to disk if the interaction has finished.
    ///
    /// The value goes to the settings first and reaches the mixer from there, so what the
    /// player hears while dragging is exactly what the file would be written with — snapping
    /// included. Applied on every change rather than only on release because the slider is
    /// the one control whose effect is the thing the player is listening to.
    fn apply_volume(&mut self, channel: VolumeChannel, value: f32, persist: bool) {
        match channel {
            VolumeChannel::Master => self.settings.set_master_volume(value),
            VolumeChannel::Sfx => self.settings.set_sfx_volume(value),
            VolumeChannel::Ui => self.settings.set_ui_volume(value),
            VolumeChannel::Ambient => self.settings.set_ambient_volume(value),
        }
        apply_volumes(&self.audio, self.settings.settings());
        if persist && self.settings.save_if_dirty() {
            let settings = self.settings.settings();
            log::info!(
                "settings: volumes saved (master {:.2}, sfx {:.2}, ui {:.2}, ambient {:.2})",
                settings.master_volume,
                settings.sfx_volume,
                settings.ui_volume,
                settings.ambient_volume,
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
        // The left button orbits only while the interface is not using it — here, that means
        // the pause menu, whose buttons it clicks. Aiming and the talent window are handled
        // by the caller, which does not reach this at all. The right button is not gated: it
        // does nothing in a menu.
        let orbit_allowed = !self.menu.is_open();
        match self.mouse_look.update(ctx.input(), orbit_allowed) {
            Some(LookChange::Began) => {
                let cursor = ctx.input().mouse_position();
                // A press over the interface is the interface's. `hovered_ability` was set
                // from this same cursor position earlier in this frame, so "was the pointer
                // on a slot" is already answered and does not have to be asked twice.
                self.click_candidate = (self.mouse_look.mode() == Some(LookMode::Orbit)
                    && self.hovered_ability.is_none())
                .then_some(cursor);
                self.click_travel = 0;
                self.begin_mouse_look(cursor);
                return;
            }
            Some(LookChange::Ended) => {
                self.end_mouse_look();
                self.finish_click();
                return;
            }
            None => {}
        }
        if !self.mouse_look.is_active() {
            return;
        }
        // The right button steering cancels the click: it is not the selection gesture, and a
        // player who pressed both is looking around rather than picking something.
        if self.mouse_look.mode() != Some(LookMode::Orbit) {
            self.click_candidate = None;
        }
        let (delta_x, delta_y) = self.motion.take();
        if delta_x != 0 || delta_y != 0 {
            self.click_travel = self
                .click_travel
                .saturating_add(delta_x.abs())
                .saturating_add(delta_y.abs());
            self.controller.rotate(delta_x as f32, delta_y as f32);
        }
        self.recenter_cursor();
    }

    /// Turns a press that did not become a drag into a selection.
    ///
    /// Called when the orbit button comes up. The gesture is the genre's: **hold** the left
    /// button to look around, **click** it to select what is under the pointer. The two are
    /// told apart by how far the cursor travelled, because that is the difference a player
    /// means, and a time-based threshold would refuse a slow, careful click on a small enemy.
    ///
    /// There are two things under the cursor, and they are asked in this order:
    ///
    /// ```text
    /// a nameplate  ->  SelectTarget { entity }   the plate already knows whose it is
    /// the world    ->  PickTarget { ray }        gameplay works out what the ray meets
    /// ```
    ///
    /// The plate first, because a plate is *interface*: it floats above its enemy, it is
    /// drawn on top of the scene, and a click that went through it to whatever body happened
    /// to be behind would be a click the player watched land somewhere else. It carries an
    /// entity rather than a ray for the same reason — the question of whose plate it is was
    /// answered when it was laid out.
    ///
    /// The world click sends a ray, not an entity: the camera and the cursor are this
    /// layer's, and which body a ray meets is gameplay's — see [`PlayerCommand::PickTarget`].
    fn finish_click(&mut self) {
        let Some(cursor) = self.click_candidate.take() else {
            return;
        };
        if self.click_travel > CLICK_SLOP_PIXELS {
            return;
        }
        // No world to click in, or a scene with no targets in it.
        if self.scene.is_collision() || self.menu.is_open() {
            return;
        }
        let pointer = (cursor.0 as f32, cursor.1 as f32);
        if let Some(entity) = self.unit_frame_at(pointer) {
            self.simulation
                .push_command(PlayerCommand::SelectTarget { entity });
            return;
        }
        if let Some(plate) = self.nameplates.at(pointer) {
            let entity = plate.entity;
            self.simulation
                .push_command(PlayerCommand::SelectTarget { entity });
            return;
        }

        let viewport = self.renderer.size();
        let view_projection = self.camera.view_projection();
        let Some((origin, direction)) =
            engine_presentation::ui::targeting::cursor_ray(pointer, &view_projection, viewport)
        else {
            return;
        };
        self.simulation
            .push_command(PlayerCommand::PickTarget { origin, direction });
    }

    /// The unit a frame under `pointer` describes, if the pointer is on one.
    ///
    /// The player's own frame and each party row. Both are how a player selects themselves
    /// without hunting for their character in the middle of a fight — the same gesture the
    /// genre has had since unit frames existed, and the reason selection is not restricted to
    /// enemies.
    ///
    /// The target frame is deliberately *not* included: clicking the frame that shows your
    /// target to select your target does nothing, and clicking it to select the *player*
    /// would make the one frame that always shows an enemy select an ally.
    fn unit_frame_at(&self, pointer: (f32, f32)) -> Option<engine_game::EntityId> {
        let scale = self.game_ui.scale();
        let viewport = self.renderer.size();
        let rect_of = |id: HudElementId| {
            self.hud_layout
                .is_enabled(id)
                .then(|| self.hud_layout.rect_of(id, scale, viewport))
        };

        if let Some(rect) = rect_of(HudElementId::Player)
            && rect.contains(pointer.0, pointer.1)
        {
            return Some(self.simulation.player().entity);
        }
        let rect = rect_of(HudElementId::Party)?;
        let row = engine_presentation::ui::unit_frame::party_row_at(
            rect,
            self.snapshot.party.len(),
            pointer,
        )?;
        self.snapshot.party.get(row).map(|member| member.entity)
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
        // The plate's box is measured from the same glyph scale the text inside it is drawn
        // at, which is what keeps the two the same size at every UI scale. Reading the scale
        // off the HUD rather than the settings means there is one answer, not two.
        let scale = self.game_ui.scale();
        let text = scale.text(UI_TEXT_SCALE);
        // The player's own nameplate size, on top of the interface scale: the first is what
        // the whole HUD is drawn at, the second is what they asked nameplates in particular
        // to be, and multiplying them is what makes a plate readable at a size the rest of
        // the interface would be unusable at.
        let nameplates = self.settings.settings().nameplates;
        let text = text * nameplates.text_scale();
        let config = NameplateConfig {
            cell_pixels: text,
            glyph_advance_pixels: engine_renderer::font::GLYPH_ADVANCE as f32 * text,
            line_advance_pixels: engine_renderer::font::LINE_ADVANCE as f32 * text,
            glyph_height_pixels: engine_renderer::font::GLYPH_HEIGHT as f32 * text,
            padding_pixels: scale.px(NameplateConfig::default().padding_pixels)
                * nameplates.text_scale(),
            ..NameplateConfig::default()
        }
        .with_settings(nameplates);

        let view_projection = self.camera.view_projection();
        self.nameplates.build(
            &self.simulation,
            &view_projection,
            viewport,
            &config,
            self.frame_seconds,
        );

        self.snapshot
            .refresh(&self.simulation, self.aiming_ability());
        self.game_ui.build_arranged(
            &self.snapshot,
            self.nameplates.plates(),
            &config,
            &self.hud_layout,
            &self.action_bars,
            &mut self.frame_scratch,
            viewport,
        );
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
            // The rectangle the tooltip points at is wherever the player put the slot, not
            // wherever the class's fixed bar used to be.
            let rect = match ability.bar_slot() {
                Some(_) => self
                    .hovered_slot_rect(viewport)
                    .unwrap_or_else(|| self.game_ui.ultimate_rect(viewport)),
                // Beside the bar: the Ultimate's own square, or one of the extras after it.
                None => match self.simulation.hero().extra_index(ability) {
                    Some(index) => self.game_ui.extra_rect(index, viewport),
                    None => self.game_ui.ultimate_rect(viewport),
                },
            };
            // The view is looked up by ability rather than by slot: a slot the current hero
            // does not fill has no view, and a tooltip for a spell the player cannot cast is
            // exactly what must not be drawn.
            if let Some(view) = self.snapshot.ability_view(ability).copied() {
                let ignite_max = self.snapshot.ignite_max;
                let mastery = self.snapshot.player.mastery;
                self.game_ui
                    .push_ability_tooltip(rect, &view, ignite_max, mastery, viewport);
            }
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
        push_character_menu(
            self.game_ui.list_mut(),
            &self.character_menu,
            &self.snapshot,
            &self.character_theme,
            &mut self.character_scratch,
            viewport,
        );

        // The slot menu over the bar it belongs to, then edit mode over everything the
        // player is arranging. Both go into the same list for the same reason the pause
        // menu does: one batch, one draw call.
        let icon_theme = *self.game_ui.theme();
        push_slot_menu(
            self.game_ui.list_mut(),
            &self.slot_menu,
            &self.action_bars,
            &self.menu_theme,
            &icon_theme,
            &mut self.hud_scratch,
            scale,
        );
        push_hud_editor(
            self.game_ui.list_mut(),
            &self.hud_editor,
            &self.hud_layout,
            &self.menu_theme,
            &mut self.hud_scratch,
            scale,
            viewport,
        );

        // The editor over the HUD it is standing in front of.
        #[cfg(feature = "editor")]
        if let Some(scene) = self.environment.as_ref() {
            engine_editor::ui::push_editor(
                self.game_ui.list_mut(),
                &self.editor,
                scene,
                &self.menu_theme,
                &mut self.hud_scratch,
                viewport,
            );
        }

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

    /// Rebuilds the terrain's mesh and puts it in the world, replacing whatever was there.
    ///
    /// One upload and one entity. Called when a level loads and again after every brush
    /// stroke, which is the only time the heights change — a stroke is a frame in the editor,
    /// where nothing else is running, and a 65×65 terrain is 8 000 triangles.
    ///
    /// A terrain that will not upload is reported and left out: the level is playable without
    /// its ground drawn, and a renderer error in the middle of a brush stroke must not take
    /// the editor down with it.
    fn rebuild_terrain(&mut self) {
        if let Some(entity) = self.terrain_entity.take() {
            self.extractor.render_world_mut().despawn(entity);
        }
        let Some(scene) = self.environment.as_ref() else {
            return;
        };
        let Some(terrain) = scene.terrain.as_ref() else {
            return;
        };
        let model = engine_assets::procedural::terrain::terrain_model(terrain, TERRAIN_MATERIAL);
        if model.triangle_count() == 0 {
            return;
        }
        let binding = match self.renderer.create_model(&model) {
            Ok(binding) => binding,
            Err(error) => {
                log::error!("terrain: could not be uploaded: {error}");
                return;
            }
        };
        // One level of detail and one material, so one part: a terrain has no LOD chain and
        // is drawn with the single surface material the level chose.
        let Some(part) = binding.lods.first().and_then(|lod| lod.parts.first()) else {
            return;
        };
        let engine_render_data::PartGeometry::Rigid(mesh) = part.geometry else {
            log::error!("terrain: built as skinned geometry, which it never is");
            return;
        };
        let bounds = binding.lods.first().map_or(part.bounds, |lod| lod.bounds);
        let world = self.extractor.render_world_mut();
        let entity = world.spawn(engine_render_data::RenderEntityDesc {
            transform: engine_core::transform::Transform::IDENTITY,
            mesh,
            material: part.material,
            bounds,
            ..Default::default()
        });
        self.terrain_entity = Some(entity);
        log::debug!(
            "terrain: {} points a side, {} triangles",
            terrain.resolution,
            model.triangle_count()
        );
    }

    /// Rebuilds the meshes the level's water is drawn as, replacing whatever was there.
    ///
    /// One upload and one entity per body. Called when a level loads and again whenever the
    /// editor reports the water changed, which is a frame in the editor — a body is two
    /// triangles, and a level has a handful of them.
    ///
    /// The yaw goes on the transform and not into the mesh, so a body that is only turned
    /// still costs an upload here; making that incremental would mean keeping a second
    /// description of which body owns which entity, and a handful of quads is not worth one.
    ///
    /// Water adds nothing to the collision world. A character walks through it because as far
    /// as movement is concerned it is not there — see `engine_environment::water`.
    fn rebuild_water(&mut self) {
        for entity in self.water_entities.drain(..) {
            self.extractor.render_world_mut().despawn(entity);
        }
        let Some(scene) = self.environment.as_ref() else {
            return;
        };
        // Cloned so the borrow of the scene ends before the renderer is touched: uploading a
        // model needs `&mut self`, and the bodies are forty bytes each.
        let bodies = scene.water.clone();
        for (index, body) in bodies.iter().enumerate() {
            let model = engine_assets::procedural::water::water_model(body, WATER_MATERIAL);
            if model.triangle_count() == 0 {
                continue;
            }
            let binding = match self.renderer.create_model(&model) {
                Ok(binding) => binding,
                Err(error) => {
                    log::error!("water[{index}]: could not be uploaded: {error}");
                    continue;
                }
            };
            let Some(part) = binding.lods.first().and_then(|lod| lod.parts.first()) else {
                continue;
            };
            let engine_render_data::PartGeometry::Rigid(mesh) = part.geometry else {
                log::error!("water[{index}]: built as skinned geometry, which it never is");
                continue;
            };
            let bounds = binding.lods.first().map_or(part.bounds, |lod| lod.bounds);
            // The centre and the yaw live here rather than in the mesh, so a body that is
            // moved or turned reuses the quad it already has.
            let transform = engine_core::transform::Transform::from_translation(body.center)
                .with_rotation(engine_core::math::Quat::from_yaw_pitch_roll(
                    body.yaw, 0.0, 0.0,
                ));
            let world = self.extractor.render_world_mut();
            let entity = world.spawn(engine_render_data::RenderEntityDesc {
                transform,
                mesh,
                material: part.material,
                bounds,
                ..Default::default()
            });
            self.water_entities.push(entity);
        }
        if !self.water_entities.is_empty() {
            log::debug!("water: {} bodies", self.water_entities.len());
        }
    }

    /// Rebuilds the markers standing on the level's spawn points.
    ///
    /// One entity per spawn point while the editor is open, and none at all otherwise — which
    /// is what `shown` says. A spawn point has no mesh of its own, so this is the only thing
    /// there is to click on and the only thing there is to see.
    ///
    /// The model is uploaded once and reused by every marker: a level with eight spawn points
    /// is eight entities sharing one mesh, the same way eight rocks of one kind are.
    #[cfg(feature = "editor")]
    fn rebuild_spawn_markers(&mut self, shown: bool) {
        for entity in self.spawn_marker_entities.drain(..) {
            self.extractor.render_world_mut().despawn(entity);
        }
        if !shown {
            return;
        }
        let Some(scene) = self.environment.as_ref() else {
            return;
        };
        let spawns = scene.spawn_points.clone();
        if spawns.is_empty() {
            return;
        }
        let model = engine_assets::procedural::starter::spawn_marker();
        let binding = match self.renderer.create_model(&model) {
            Ok(binding) => binding,
            Err(error) => {
                log::error!("spawn markers: could not be uploaded: {error}");
                return;
            }
        };
        let Some(part) = binding.lods.first().and_then(|lod| lod.parts.first()) else {
            return;
        };
        let engine_render_data::PartGeometry::Rigid(mesh) = part.geometry else {
            log::error!("spawn markers: built as skinned geometry, which they never are");
            return;
        };
        let bounds = binding.lods.first().map_or(part.bounds, |lod| lod.bounds);
        for spawn in &spawns {
            let transform = engine_core::transform::Transform::from_translation(spawn.position)
                .with_rotation(engine_core::math::Quat::from_yaw_pitch_roll(
                    spawn.yaw, 0.0, 0.0,
                ));
            let world = self.extractor.render_world_mut();
            let entity = world.spawn(engine_render_data::RenderEntityDesc {
                transform,
                mesh,
                material: part.material,
                bounds,
                ..Default::default()
            });
            self.spawn_marker_entities.push(entity);
        }
    }

    /// Rebuilds the static collision world from the environment scene as it stands now.
    ///
    /// The floor and the boundary first, then every blocking placement, then one broad-phase
    /// build — the identical sequence `engine_game::arena::environment_arena` runs at load,
    /// called rather than re-written, so an edited level is solid in exactly the way a loaded
    /// one is. It costs one pass over a few hundred shapes and is paid on the frames an
    /// author is editing, where nothing else is running.
    #[cfg(feature = "editor")]
    fn rebuild_environment_collision(&mut self) {
        let Some(scene) = self.environment.as_ref() else {
            return;
        };
        // Zero dummies: the configuration is read here only for the floor height and the
        // boundary, and neither depends on how large the group is.
        let config = ArenaConfig::from_environment(scene, 0);
        // The level's ground may have moved: a brush stroke changes where the bottom of the
        // world is, and with it what counts as having fallen out of it.
        self.simulation.set_void_height(config.void_height);
        let world = self.simulation.collision_world_mut();
        arena::build_collision(world, &config);
        arena::add_terrain_collision(world, scene);
        let statics = arena::add_environment_collision(world, scene);
        world.rebuild_broad_phase();
        log::debug!("editor: collision rebuilt, {statics} shape(s) from the scene");
    }

    /// One frame of the level editor. Returns whether it owned the frame.
    ///
    /// `F6` opens it and closes it. While it is open the simulation does not advance and no
    /// gameplay command is issued, so the only things that move are the camera and whatever
    /// the author is dragging.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the camera cannot be written, which a degenerate aspect
    /// ratio causes.
    #[cfg(feature = "editor")]
    fn update_editor(&mut self, ctx: &mut EngineContext) -> Result<bool, EngineError> {
        let viewport = self.renderer.size();
        if !self.editor.is_active() {
            let requested = std::mem::take(&mut self.editor_open_requested);
            if !requested && !engine_editor::wants_to_enter(ctx.input()) {
                return Ok(false);
            }
            let Some((name, count)) = self
                .environment
                .as_ref()
                .map(|scene| (scene.name.to_string(), scene.len()))
            else {
                log::info!("editor: this scene has no environment to edit");
                return Ok(false);
            };
            // A pointer that is captured for mouse-look cannot pick anything, and a hidden
            // one cannot be aimed. The editor is a pointer interface, exactly as edit-HUD
            // and the slot menu are.
            let focus =
                engine_presentation::extract::interpolated_player_position(&self.simulation);
            self.cancel_mouse_look();
            self.editor.set_scale(self.game_ui.scale(), viewport);
            let editor_started = Instant::now();
            self.editor.enter(focus, &name, viewport);
            crate::startup::stage_from("editor init", editor_started);
            if let Some(path) = self.editor_save_path.clone() {
                self.editor.set_save_path(path);
            }
            log::info!("editor: editing '{name}' ({count} objects); F6 goes back to playing");
            // The signposts go up with the editor and come down with it.
            self.rebuild_spawn_markers(true);
            return Ok(true);
        }

        let ui_input = UiInput::from_input(ctx.input());
        let view_projection = self.camera.view_projection();
        let seconds = ctx.time().real_delta().as_secs_f32();
        let mut changed = false;
        let mut terrain_changed = false;
        let mut water_changed = false;
        if let Some(scene) = self.environment.as_mut() {
            match self.editor.update(
                ctx.input(),
                &ui_input,
                scene,
                &view_projection,
                viewport,
                seconds,
            ) {
                engine_editor::EditorOutcome::SceneChanged => changed = true,
                // The ground moved. One mesh and one collision surface, and none of the
                // placements: re-placing a few hundred render entities on every frame of a
                // brush stroke is the cost this outcome exists to avoid.
                engine_editor::EditorOutcome::TerrainChanged => terrain_changed = true,
                // A quad moved, grew or arrived. Two triangles to upload, and none of the
                // placements or the collision world to touch: water blocks nothing.
                engine_editor::EditorOutcome::WaterChanged => water_changed = true,
                engine_editor::EditorOutcome::Left => {
                    // The static world is rebuilt on the way out as well as on every
                    // change, so that leaving the editor lands the author in the level
                    // they just built rather than in the one that was loaded.
                    self.rebuild_environment_collision();
                    self.rebuild_spawn_markers(false);
                    log::info!("editor: back to playing; collision describes what is on screen");
                    return Ok(false);
                }
                engine_editor::EditorOutcome::Idle => {}
            }
        }

        // One code path for "the scene changed": clear the placements and put them back.
        // A few hundred respawns is a load-time-sized cost paid on the frames an author is
        // actually editing, and an incremental spawn path is a second description of the
        // scene that can disagree with the first.
        // An edit re-places the level: models the browser has never uploaded, a terrain, a
        // few quads of water, the spawn markers. One batch around the lot, for the reason
        // startup has one — a mesh that waits for the GPU on its own is a stall an author
        // feels on every placement.
        if changed || terrain_changed || water_changed {
            self.renderer.begin_upload_batch()?;
        }
        if changed && let Some(scene) = self.environment.as_ref() {
            // An asset placed from the browser has no model uploaded yet: the library went up
            // at startup and this file was discovered afterwards. Uploading here rather than
            // when the browser is scanned means a directory of two hundred assets costs
            // nothing until one of them is actually placed.
            upload_missing_models(&mut self.renderer, self.placer.visuals_mut(), scene);
            let world = self.extractor.render_world_mut();
            self.placer.clear(world);
            self.placer
                .place(world, scene, controller_eye_guess(scene), LOD_BIAS);
            // What is drawn and what is solid come from one description of the level, and
            // they are rebuilt together. An author who places a wall and walks into it
            // expects a wall; the alternative is an editor whose work can only be tested by
            // relaunching the game.
            self.rebuild_environment_collision();
            self.rebuild_terrain();
            self.rebuild_water();
        }
        if terrain_changed {
            self.rebuild_terrain();
            self.rebuild_environment_collision();
        }
        if water_changed {
            self.rebuild_water();
            // The same outcome carries a spawn point that moved: both are level furniture with
            // one or two entities behind them, and telling them apart would buy nothing.
            self.rebuild_spawn_markers(true);
        }
        if changed {
            self.rebuild_spawn_markers(true);
        }
        if changed || terrain_changed || water_changed {
            self.renderer.end_upload_batch()?;
        }

        let (width, height) = self.renderer.size();
        if width > 0 && height > 0 {
            self.editor
                .apply_camera(&mut self.camera, aspect_of(width, height))?;
        }
        if self.environment.is_some() {
            self.placer.update_lod(
                self.extractor.render_world_mut(),
                self.camera.position(),
                LOD_BIAS,
            );
        }
        self.build_ui();
        Ok(true)
    }

    /// Puts the area indicator on the ground, or takes it away.
    ///
    /// Two flat meshes a few centimetres up: a dim fill for the footprint and a bright ring
    /// for its edge, both scaled to the ability's own radius so what is shown is exactly
    /// what will be hit. The ring breathes slowly, which is what makes it read as a live
    /// indicator rather than as a decal somebody left on the floor.
    /// Character test scene only: keys 1 to 6 hold the character in one animation.
    ///
    /// The point of the scene. Judging a run cycle by running means chasing the character
    /// with the camera while it covers ground; holding it in the run animation on the spot
    /// means looking at the thing being judged. `0` releases the override and the character
    /// goes back to reflecting gameplay, which is what proves the override is an override.
    fn update_animation_override(&mut self, ctx: &mut EngineContext) {
        use engine_assets::AnimationSlot;

        const BINDINGS: [(KeyCode, Option<AnimationSlot>); 8] = [
            (KeyCode::Digit1, Some(AnimationSlot::Idle)),
            (KeyCode::Digit2, Some(AnimationSlot::Walk)),
            (KeyCode::Digit3, Some(AnimationSlot::Run)),
            (KeyCode::Digit4, Some(AnimationSlot::Jump)),
            (KeyCode::Digit5, Some(AnimationSlot::Cast)),
            (KeyCode::Digit6, Some(AnimationSlot::CastHeavy)),
            (KeyCode::Digit7, Some(AnimationSlot::Fall)),
            (KeyCode::Digit0, None),
        ];

        let Some(visual) = self.extractor.player_visual_mut() else {
            return;
        };
        for (key, slot) in BINDINGS {
            if ctx.input().was_key_pressed(key) {
                visual.force_slot(slot);
            }
        }
    }

    /// Plays catalogue effects around the player at a fixed rate, for measurement.
    ///
    /// **Presentation only.** It starts no ability, deals no damage and touches no gameplay
    /// state, so the frame time it moves is the cost of *drawing* effects. That separation
    /// is the whole point of the control: a stress driver that cast abilities would measure
    /// the ability system's throughput and call the result a VFX number.
    fn drive_vfx_stress(&mut self, delta: f32) {
        use engine_vfx::{VfxId, VfxSpawn};

        if self.vfx_stress <= 0.0 {
            return;
        }
        self.vfx_stress_debt += self.vfx_stress * delta;
        let owed = self.vfx_stress_debt as u32;
        self.vfx_stress_debt -= f64::from(owed) as f32;

        let centre = self.simulation.player_position();
        let player = self.simulation.player().entity.index() as u32;
        for _ in 0..owed {
            let index = self.vfx_stress_next % engine_vfx::VfxId::ALL.len();
            self.vfx_stress_next = self.vfx_stress_next.wrapping_add(1);
            let effect = engine_vfx::VfxId::ALL[index];

            // Scattered around the player on a ring, so the load is spread across the view
            // rather than stacked on one pixel — overdraw at one point is a different
            // question from many effects on screen, and the honest measurement is the
            // second one.
            let angle = index as f32 * 2.399_963 + self.simulation.time() as f32;
            let distance = 4.0 + (index % 5) as f32 * 2.5;
            let position = centre + Vec3::new(angle.cos() * distance, 0.0, angle.sin() * distance);

            let attached = matches!(effect, VfxId::Ignited | VfxId::Shield | VfxId::CastCharge);
            let mut spawn = VfxSpawn::at(effect, position)
                .facing(angle)
                .with_radius(VFX_DEMO_RADIUS)
                .with_length(VFX_DEMO_RADIUS * 3.0);
            if attached {
                spawn = spawn
                    .with_radius(1.0)
                    .raised(1.1)
                    .attached_to(player)
                    .lasting(2.0);
            }
            self.vfx.system_mut().spawn(spawn);
        }
    }

    /// VFX test bed only: one key per effect in the catalogue.
    ///
    /// Digits 1 to 0 play the first ten effects and Z, X, C the last three, each in front of
    /// the character at a middling arena radius. The point of the scene is that judging an
    /// explosion means playing it on demand from a chosen angle rather than waiting for a
    /// fight to produce one.
    fn update_vfx_demo(&mut self, ctx: &mut EngineContext) {
        use engine_vfx::{VfxId, VfxSpawn};

        const KEYS: [KeyCode; 13] = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
            KeyCode::Digit0,
            KeyCode::Z,
            KeyCode::X,
            KeyCode::C,
        ];

        let body = self.simulation.player_body().copied();
        let Some(body) = body else {
            return;
        };
        let yaw = body.facing_yaw;
        let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        let anchor = body.position + forward * VFX_DEMO_DISTANCE;

        for (index, key) in KEYS.into_iter().enumerate() {
            if !ctx.input().was_key_pressed(key) {
                continue;
            }
            let Some(effect) = VfxId::ALL.get(index).copied() else {
                continue;
            };
            // Attached effects are given the player to ride, because an aura played at a
            // point on the floor is an aura nobody can judge.
            let attach = matches!(effect, VfxId::Ignited | VfxId::Shield | VfxId::CastCharge);
            let mut spawn = VfxSpawn::at(effect, if attach { body.position } else { anchor })
                .facing(yaw)
                .with_radius(VFX_DEMO_RADIUS)
                .with_length(VFX_DEMO_RADIUS * 3.0);
            if attach {
                spawn = spawn
                    .with_radius(1.0)
                    .raised(1.1)
                    .attached_to(self.simulation.player().entity.index() as u32)
                    .lasting(6.0);
            }
            self.vfx.system_mut().spawn(spawn);
            #[cfg(feature = "debug-overlay")]
            {
                self.last_effect = Some(effect);
            }
            log::info!("vfx demo: {}", effect.name());
        }
    }

    /// The audio test bed's controls.
    ///
    /// Walking is the instrument: a sound is *dropped at a place* and judged by walking
    /// towards it, away from it and around it, which is the only way to hear attenuation and
    /// panning for what they are. Playing everything on the player would test the mixer and
    /// nothing else.
    ///
    /// ```text
    /// Z / X     previous / next sound in the catalogue
    /// C         drop the selected sound in front of you
    /// V         play it on you, with no place at all (what UI sounds do)
    /// B         stop everything that is playing
    /// N         eight at once, to hear the limiter and the priority model decide
    /// 1 - 0     jump straight to one of the first ten sounds
    /// ```
    fn update_audio_demo(&mut self, ctx: &mut EngineContext) {
        use engine_audio::{SoundCue, SoundId};

        let body = self.simulation.player_body().copied();
        let Some(body) = body else {
            return;
        };
        let yaw = body.facing_yaw;
        let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        let anchor = body.position + forward * AUDIO_DEMO_DISTANCE + Vec3::new(0.0, 1.0, 0.0);

        // The first ten sounds get a digit each; everything past them is reached by
        // stepping. Thirty-nine keys would be a keyboard map nobody could remember.
        let mut selection = self.audio_demo_selection;
        for number in 0..10 {
            let Some(key) = KeyCode::digit_key((number + 1) % 10) else {
                continue;
            };
            if ctx.input().was_key_pressed(key) {
                selection = number as usize;
            }
        }
        if ctx.input().was_key_pressed(KeyCode::Z) {
            selection = (selection + engine_audio::SOUND_COUNT - 1) % engine_audio::SOUND_COUNT;
        }
        if ctx.input().was_key_pressed(KeyCode::X) {
            selection = (selection + 1) % engine_audio::SOUND_COUNT;
        }
        let sound = SoundId::ALL
            .get(selection)
            .copied()
            .unwrap_or(SoundId::UiClick);
        self.audio_demo_selection = selection;

        let Ok(mut audio) = self.audio.system().lock() else {
            return;
        };
        if ctx.input().was_key_pressed(KeyCode::C) {
            audio.play(SoundCue::at(sound, anchor));
            log::info!(
                "audio demo: {} at {:.0} m, {} priority {}",
                sound.name(),
                AUDIO_DEMO_DISTANCE,
                sound.desc().category.as_str(),
                sound.desc().priority,
            );
        }
        if ctx.input().was_key_pressed(KeyCode::V) {
            audio.play(SoundCue::ui(sound));
            log::info!("audio demo: {} with no position", sound.name());
        }
        if ctx.input().was_key_pressed(KeyCode::B) {
            audio.stop_all();
            log::info!("audio demo: everything stopped");
        }
        if ctx.input().was_key_pressed(KeyCode::N) {
            // Eight of the same thing at once, spread around the player: what the per-sound
            // cap and the minimum interval do is audible here and nowhere else.
            for step in 0..8 {
                let angle = step as f32 / 8.0 * std::f32::consts::TAU;
                let around = body.position
                    + Vec3::new(angle.cos(), 0.0, angle.sin()) * AUDIO_DEMO_DISTANCE
                    + Vec3::new(0.0, 1.0, 0.0);
                audio.play(SoundCue::at(sound, around));
            }
            let stats = audio.stats();
            log::info!(
                "audio demo: eight {} → {} playing, {} refused",
                sound.name(),
                audio.playing(sound),
                stats.suppressed_interval + stats.suppressed_limit + stats.dropped,
            );
        }
    }

    fn update_targeting_decal(&mut self) {
        let Some(target) = self.targeting.active() else {
            if let Some(decal) = self.targeting_decal.take() {
                let world = self.extractor.render_world_mut();
                for entity in decal.border.into_iter().chain(decal.glow) {
                    world.despawn(entity);
                }
            }
            self.affected.clear();
            return;
        };

        let visuals = self.decal_visuals;
        let (border_material, glow_material) = if target.valid {
            (visuals.border_valid, visuals.glow_valid)
        } else {
            (visuals.border_invalid, visuals.glow_invalid)
        };

        // The surface the outline is laid on. A level with a height field is sampled; one
        // without gets an exact plane rather than an approximation of one.
        let ground = match self
            .environment
            .as_ref()
            .and_then(|scene| scene.terrain.as_ref())
        {
            Some(terrain) => GroundSurface::Terrain(terrain),
            None => GroundSurface::Plane(engine_presentation::ui::targeting::GROUND_HEIGHT),
        };

        // The border's outer rim is the ability's edge, so the line the player aims with is
        // the boundary the executor tests. The band sits inside it, starting where the
        // border ends, so the two touch without overlapping.
        let border = DecalBand::edged(DECAL_BORDER_WIDTH, DECAL_BORDER_LIFT);
        let glow = DecalBand::edged(DECAL_GLOW_WIDTH, DECAL_GLOW_LIFT)
            .with_inset(DECAL_BORDER_WIDTH + DECAL_GLOW_WIDTH * 0.5);

        // Three shapes, one system, and now one geometry too: a circle sits under the cursor
        // while a cone and a line are anchored on the caster, and all of them are an outline
        // resampled onto the ground. All are a fixed size - an indicator that breathed would
        // be lying about the area it covers.
        engine_presentation::ui::ground_decal::build_band(
            target.shape,
            target.anchor,
            target.facing,
            border,
            &ground,
            &mut self.decal_scratch,
            &mut self.decal_border,
        );
        engine_presentation::ui::ground_decal::build_band(
            target.shape,
            target.anchor,
            target.facing,
            glow,
            &ground,
            &mut self.decal_scratch,
            &mut self.decal_glow,
        );

        let mut decal = self.targeting_decal.take().unwrap_or_default();
        let world = self.extractor.render_world_mut();
        sync_decal_band(
            world,
            &mut decal.border,
            &self.decal_border,
            visuals.quad_mesh,
            border_material,
            visuals.quad_bounds,
        );
        sync_decal_band(
            world,
            &mut decal.glow,
            &self.decal_glow,
            visuals.quad_mesh,
            glow_material,
            visuals.quad_bounds,
        );
        self.targeting_decal = Some(decal);

        // Everything the shape currently covers, so the interface can mark it. The shape
        // decides, not the app: a future cone marks what a cone covers with no change here.
        self.affected.clear();
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
            // Edge-corrected with the body's own radius, exactly as the executor collects
            // its hits: the ring on screen and the enemies the ability catches have to be
            // the same set, or the interface is lying while the player aims.
            if target
                .shape
                .contains_body(target.anchor, target.facing, position, world.radius(id))
            {
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
            let scale = self.game_ui.scale();
            let size = scale.px(AFFECTED_MARK_SIZE);
            let rect = UiRect::new(screen.0 - size * 0.5, screen.1 - size * 0.5, size, size);
            self.game_ui
                .list_mut()
                .outline(rect, scale.px(3.0), [1.0, 0.72, 0.20, 0.95]);
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
            self.game_ui.scale().text(UI_TEXT_SCALE),
        );
        // Without the developer feature there is nothing to label: the geometry is not
        // drawn either, so a marker would point at empty space.
        #[cfg(not(feature = "debug-overlay"))]
        let _ = scene;
    }

    /// Logs a combat and performance report if the reporting interval has elapsed.
    ///
    /// The whole report is **one** log record. It used to be fifteen to thirty-five, and
    /// each one was its own unbuffered write to stderr — on a Windows console that is
    /// blocking inter-process communication, and thirty of them in a row is a visible
    /// hitch once a second. Built into a reusable buffer, so the report costs a formatted
    /// string and nothing else.
    fn maybe_report(&mut self, ctx: &EngineContext) {
        use std::fmt::Write as _;

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
        // One check for the whole report, since it is one record: nothing below is built
        // when the level filters it out.
        if !engine_core::logging::enabled(log::Level::Info, module_path!()) {
            return;
        }
        // Taken rather than borrowed: the report reads the rest of `self` while it is
        // being built, and the buffer goes back at the end with its capacity intact.
        let mut report = std::mem::take(&mut self.report);

        // The last advance that ticked, not the last advance: most frames at these rates
        // run no ticks, and their zero would read as "the simulation is free".
        let stats = self.simulation.last_tick_stats();
        let meter = self.simulation.meter();
        let cull = self.renderer.cull_stats();
        let ui = self.renderer.ui_stats();
        // Writing to a `String` cannot fail, here and below.
        let _ = write!(
            report,
            "CPU mean {:.3} ms ({:.0} FPS) p99 {:.3} | GPU ",
            summary.cpu.mean_ms,
            summary.cpu.mean_fps(),
            summary.cpu.p99_ms,
        );
        match summary.gpu {
            Some(gpu) => {
                let _ = write!(report, "mean {:.3} p99 {:.3} ms", gpu.mean_ms, gpu.p99_ms);
            }
            None => report.push_str("N/A"),
        }
        let _ = write!(
            report,
            " | sim {:.3} ms ({} ticks) | \
             extract {:.3} ms | vfx {:.3} ms | audio {:.3} ms | cull {:.3} ms | ui {:.3} ms ({} quads, {} cmds) | visible {} | \
             draws {} ({} skinned) | tris {}",
            stats.total_ms(),
            stats.ticks,
            self.extract_ms,
            self.vfx_ms,
            self.audio_ms,
            self.renderer.last_cull_ms(),
            self.ui_ms + self.renderer.ui_build_ms(),
            ui.quads,
            ui.commands,
            cull.visible,
            self.renderer.draw_calls_last_frame(),
            self.renderer.skinned_draws_last_frame(),
            self.renderer.triangles_last_frame(),
        );
        // The light grid on the same line as the lights it culls: "how many lights" and
        // "how many does a pixel actually evaluate" are the pair that says whether the
        // clustering is doing anything, and either alone says nothing.
        let clusters = self.renderer.light_cluster_stats();
        let _ = write!(
            report,
            "
  lights: {} local ({} dropped) | clusters {} occupied, {} assignments, worst {} per cluster",
            clusters.lights,
            clusters.dropped,
            clusters.occupied,
            clusters.assignments,
            clusters.max_per_cluster,
        );

        // Shadows on their own line: the stage has its own CPU time, its own GPU time and
        // its own draw calls, and folding them into the frame's totals is what makes "the
        // frame got slower" impossible to attribute.
        let shadow = self.renderer.shadow_stats();
        if shadow.enabled {
            let _ = write!(
                report,
                "\n  shadows: {} cascade(s) at {} | cpu {:.3} ms | gpu ",
                shadow.cascade_count, shadow.resolution, shadow.cpu_ms,
            );
            match shadow.gpu_ms {
                Some(milliseconds) => {
                    let _ = write!(report, "{milliseconds:.3} ms");
                }
                None => report.push_str("N/A"),
            }
            let _ = write!(
                report,
                " | {} draws, {} tris, \
                 {} casters ({} dropped) | local {} view(s), {} refused | atlas {}x{}",
                shadow.draw_calls(),
                shadow.triangles(),
                shadow.casters(),
                shadow.lod_counts[3],
                shadow.local_views,
                shadow.local_lights_refused,
                shadow.atlas_size.0,
                shadow.atlas_size.1,
            );
        }
        let vfx = self.vfx.stats();
        let _ = write!(
            report,
            "\n  vfx: {} effects, {} particles (peak {}), {} mesh draws, {} entities |              dropped {} effects / {} particles | drawn {}",
            vfx.system.instances,
            vfx.system.particles,
            vfx.system.peak_particles,
            vfx.system.mesh_draws,
            vfx.entities,
            vfx.system.dropped_instances + vfx.refused,
            vfx.system.dropped_particles,
            self.renderer.particles_last_frame(),
        );
        // Audio, with every refusal counted: what is playing, what was thinned by the
        // limiter, and what the pool had no room for. A wall of identical sounds shows up
        // here as a large `thinned` and a steady `voices`, which is the system working.
        //
        // The statistics are copied out and the lock released before anything is
        // formatted: the audio mutex is shared with the real-time cpal callback, and
        // holding it across a `write!` would stall the sound to print a line about it.
        let audio_stats = self
            .audio
            .system()
            .lock()
            .ok()
            .map(|audio| self.audio_director.stats(&audio));
        if let Some(stats) = audio_stats {
            let _ = write!(
                report,
                "\n  audio: {} voices (peak {} of {}) | started {} | thinned {} | dropped {} | stolen {} | missing {} | limited {} | {} loops, {} bolts | device {}",
                stats.system.active,
                stats.system.peak_active,
                stats.system.capacity,
                stats.system.started,
                stats.system.suppressed_interval + stats.system.suppressed_limit,
                stats.system.dropped,
                stats.system.stolen,
                stats.system.missing,
                stats.system.limited,
                stats.loops,
                stats.bolts,
                if self.audio.is_open() {
                    "open"
                } else {
                    "silent"
                },
            );
        }
        // The fight, if there is one: what every enemy is doing, who it is doing it to and
        // how angry it is. Developer-only, and in the log rather than over the game, exactly
        // as the audio readout is.
        if self.simulation.enemy_count() > 0 {
            let encounters = self.simulation.encounters();
            let _ = write!(
                report,
                "\n  encounters: {} | {} enemies | {} hazards | player {:.0} hp, took {:.0}",
                encounters.len(),
                self.simulation.enemy_count(),
                self.simulation.enemy_hazards().len(),
                self.simulation
                    .world()
                    .health(self.simulation.player().entity)
                    .map_or(0.0, engine_game::Health::current),
                self.simulation.meter().total_damage_taken(),
            );
            // One line per pack: what it is called, where it is in its life, and how much of
            // it is still standing. A dungeon's nine packs fit on nine lines; a single
            // encounter scene prints one and reads exactly as it did before.
            for encounter in encounters {
                let alive = encounter
                    .members()
                    .iter()
                    .filter(|member| self.simulation.world().is_alive(**member))
                    .count();
                let _ = write!(
                    report,
                    "\n    pack {:<26} {:<11} {alive}/{} alive | {} resets",
                    encounter.name(),
                    encounter.state().as_str(),
                    encounter.len(),
                    encounter.resets(),
                );
            }
            // The per-enemy roll-call is the bulk of the report and is rarely what is being
            // read, so it comes in at `Debug`: absent from a release build, and off by
            // default elsewhere. Only what is doing something, once there are more than a
            // handful: a dungeon holds twenty-two enemies and twenty of them are standing
            // still.
            if log::log_enabled!(log::Level::Debug) {
                let verbose = self.simulation.enemy_count() <= 8;
                for (entity, brain) in self.simulation.enemies() {
                    if !verbose && brain.state() == engine_game::enemy::brain::AiState::Idle {
                        continue;
                    }
                    let position = self
                        .simulation
                        .world()
                        .position(entity)
                        .unwrap_or(Vec3::ZERO);
                    let target = brain.target();
                    let distance = target
                        .and_then(|target| self.simulation.world().position(target))
                        .map(|other| engine_game::world::horizontal_distance(position, other));
                    let _ = write!(
                        report,
                        "\n    {} {:<7} {:>3.0} hp | {} | target {} at ",
                        brain.archetype().name,
                        brain.state().as_str(),
                        self.simulation
                            .world()
                            .health(entity)
                            .map_or(0.0, engine_game::Health::current),
                        brain.phase_name(),
                        target.map_or(-1, |target| target.index() as i64),
                    );
                    match distance {
                        Some(metres) => {
                            let _ = write!(report, "{metres:.1}");
                        }
                        None => report.push('-'),
                    }
                    let _ = write!(report, " m | threat {} | ", brain.threat().len());
                    match brain.cast() {
                        Some(cast) => {
                            let _ =
                                write!(report, "{} {:.1}s", cast.ability.name(), cast.remaining());
                        }
                        None => report.push('-'),
                    }
                    if !brain.threat().is_empty() {
                        report.push_str(" *");
                    }
                }
            }
        }
        if let Some(scene) = self.environment.as_ref() {
            let environment = self.placer.stats();
            let _ = write!(
                report,
                "\n  environment '{}': {} objects, {} entities, {} models, {} lod changes",
                scene.name,
                environment.objects,
                environment.entities,
                self.placer.visuals().len(),
                environment.lod_changes,
            );
        }
        let _ = write!(
            report,
            "\n  combat: {:.0} damage | {:.0} DPS over {:.1} s | {} deaths | {} events",
            meter.total_damage(),
            meter.dps(),
            meter.combat_seconds(),
            meter.deaths(),
            self.simulation.event_stream().total_recorded(),
        );
        log::info!("{report}");
        report.clear();
        self.report = report;
    }

    /// Appends the shadow stage's statistics to the developer overlay.
    ///
    /// Everything §18 and §19 ask to be inspectable, in the order it is useful: what the
    /// stage is configured as, what it cost, and what it drew — per cascade, because "the
    /// shadow pass is expensive" and "the last cascade is drawing the whole world" are
    /// different problems with the same symptom.
    #[cfg(feature = "debug-overlay")]
    fn append_shadow_lines(&mut self) {
        use std::fmt::Write as _;

        let stats = self.renderer.shadow_stats();
        let quality = self.settings.settings().graphics.shadows;
        let mut push = |text: String| self.overlay_lines.push(text);

        let mut line = String::with_capacity(64);
        let _ = write!(
            line,
            "SHADOWS {}  {} CASC {}  ATLAS {}X{} ({:.0} MB)",
            quality.as_str().to_uppercase(),
            stats.cascade_count,
            stats.resolution,
            stats.atlas_size.0,
            stats.atlas_size.1,
            stats.atlas_bytes as f64 / (1024.0 * 1024.0),
        );
        push(std::mem::take(&mut line));

        let _ = write!(
            line,
            "  CPU {:>5.2} MS  GPU {}  DRAWS {}  TRIS {}",
            stats.cpu_ms,
            match stats.gpu_ms {
                Some(milliseconds) => format!("{milliseconds:>5.2} MS"),
                None => " N/A".to_owned(),
            },
            stats.draw_calls(),
            stats.triangles(),
        );
        push(std::mem::take(&mut line));

        let _ = write!(
            line,
            "  CASTERS {}  SKINNED {}  DROPPED {}  PREPASS {}",
            stats.casters(),
            stats.skinned_draws,
            stats.lod_counts[3],
            if stats.prepass { "ON" } else { "OFF" },
        );
        push(std::mem::take(&mut line));

        for index in 0..stats.cascade_count as usize {
            let pass = stats.cascades[index];
            let _ = write!(
                line,
                "  C{index} <{:>5.1} M  {:>4} CASTERS  {:>3} DRAWS  {:>6} TRIS",
                stats.splits[index], pass.casters, pass.draw_calls, pass.triangles,
            );
            push(std::mem::take(&mut line));
        }

        if stats.local_views > 0 || stats.local_lights_refused > 0 {
            let _ = write!(
                line,
                "  LOCAL {} LIGHT(S) IN {} VIEW(S), {} REFUSED  {} DRAWS",
                stats.local_lights,
                stats.local_views,
                stats.local_lights_refused,
                stats.local.draw_calls,
            );
            push(std::mem::take(&mut line));
        }

        // What the LOD policy actually did, which is the number that says whether it is
        // earning its place: level 0 is full detail, level 3 is "not drawn at all".
        let _ = write!(
            line,
            "  LOD {} / {} / {} / {} (FULL / REDUCED / SIMPLE / NONE)",
            stats.lod_counts[0], stats.lod_counts[1], stats.lod_counts[2], stats.lod_counts[3],
        );
        push(line);
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
        // The line-of-sight readout rides with the collision debug drawing, on the same key:
        // the ray is drawn in the world by F3, and this says in words what it found. Off
        // while that view is off, so the ordinary overlay is still the frame rate and
        // nothing else.
        // The shadow readout rides with the shadow debug view, on the same key: F7 tints the
        // world by cascade and this says in numbers what that view is showing. Off while the
        // view is off, so the ordinary overlay stays the frame rate and nothing else.
        if (1..=4).contains(&self.shadow_debug_view) {
            self.append_shadow_lines();
        }
        if self.collision_debug.enabled() {
            engine_presentation::los_debug::append_lines(&mut self.overlay_lines, &self.simulation);
        }
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

        if let Some((started, first_frame)) = self.measured_from {
            let measured = frames.saturating_sub(first_frame);
            let span = started.elapsed().as_secs_f64();
            log::info!(
                "measured window: {measured} frames in {span:.3} s ({:.0} FPS, warm-up {}                  frame(s) discarded)",
                if span > 0.0 {
                    measured as f64 / span
                } else {
                    0.0
                },
                self.warmup_frames,
            );
        }

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
        let clusters = self.renderer.light_cluster_stats();
        log::info!(
            "final light clusters: {} local light(s), {} dropped, {} of {} clusters occupied,              worst {} light(s) in one cluster",
            clusters.lights,
            clusters.dropped,
            clusters.occupied,
            engine_renderer::light_cluster::CLUSTER_COUNT,
            clusters.max_per_cluster,
        );
        let shadow = self.renderer.shadow_stats();
        if shadow.enabled {
            log::info!(
                "final shadow budget: {} local view(s) granted, {} light(s) refused,                  {} caster(s), {} dropped by LOD",
                shadow.local_views,
                shadow.local_lights_refused,
                shadow.casters(),
                shadow.lod_counts[3],
            );
        }
        if let Some(stats) = self.shadow_gpu.stats() {
            log::info!(
                "final shadow-stage GPU: mean {:.3} | p50 {:.3} | p95 {:.3} ms | n={}",
                stats.mean_ms,
                stats.p50_ms,
                stats.p95_ms,
                stats.count,
            );
        }
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
    /// Advances the resolution benchmark by one frame, if one is running.
    ///
    /// Everything the benchmark needs about the frame is gathered here and handed over as
    /// one report, so the state machine itself needs no opinion about the renderer. What
    /// comes back is a window request, a metrics boundary, or the end of the matrix.
    fn advance_benchmark(&mut self, ctx: &mut EngineContext) {
        let Some(benchmark) = self.benchmark.as_mut() else {
            return;
        };

        let descriptors = self.renderer.descriptor_stats();
        let report = FrameReport {
            // The swap chain's own size, not the window's: the render target is what was
            // measured, and a window whose client area disagreed with it would be the bug
            // this field exists to catch.
            actual_size: self.renderer.size(),
            pass_ms: self.renderer.last_pass_ms(),
            draw_calls: self.renderer.draw_calls_last_frame(),
            textures: self.renderer.texture_requests(),
            unique_textures: self.renderer.texture_count(),
            descriptors: (descriptors.used, descriptors.capacity),
        };

        let mut boundary = Boundary::None;
        let action = benchmark.frame(&report, &mut boundary);

        match boundary {
            // The window opens here rather than at the resize, so the percentiles cover
            // steady-state frames at this resolution and nothing else: no swap-chain
            // recreation, no first-frame pipeline work, no timestamps still describing the
            // previous size.
            Boundary::OpenMeasurement => {
                ctx.metrics_mut().clear();
                self.shadow_gpu.clear();
            }
            Boundary::CloseMeasurement => {
                // Recomputed rather than read from the cached summary, which can be a
                // quarter of a second old — the same thing the end-of-run report does.
                ctx.metrics_mut().refresh();
                let summary = ctx.metrics().summary();
                benchmark.record_engine_stats(
                    summary.as_ref().map(|summary| summary.cpu),
                    summary.and_then(|summary| summary.gpu),
                );
            }
            Boundary::None => {}
        }

        match action {
            Action::Resize(width, height) => {
                // Asked of the window, not of the renderer: the resize then arrives as the
                // ordinary `WM_SIZE`, through the ordinary event path, and recreates exactly
                // what a player dragging the window edge would recreate.
                if let Err(error) = self.window.set_client_size(width, height) {
                    log::error!("benchmark: could not resize to {width}x{height}: {error}");
                    ctx.request_exit();
                }
            }
            Action::Continue => {}
            Action::Finished => {
                log::info!(
                    "benchmark: the matrix is done\n{}",
                    benchmark.report().trim_end()
                );
                ctx.request_exit();
            }
        }
    }
}

impl Application for App {
    type Error = EngineError;

    /// Drains window messages and applies them.
    fn process_events(&mut self, ctx: &mut EngineContext) -> Result<(), EngineError> {
        // The device the opening thread went to fetch, if it has arrived. Answered here
        // rather than waited for at startup: the player's volumes reach the runtime the
        // moment it has a bank to apply them to, and a frame or two of silence costs
        // nothing that a launch is 120 ms shorter cannot pay for.
        if let Some(opened) = self.audio.poll_opened() {
            apply_volumes(&self.audio, self.settings.settings());
            log::info!(
                "audio: {} at {} Hz, {} channel(s)",
                if opened.open { "playing" } else { "silent" },
                opened.sample_rate,
                opened.channels
            );
        }
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

        // The warm-up boundary: everything recorded so far described the load rather than
        // the frame, so it is discarded here rather than being averaged with what follows.
        if self.warmup_frames > 0 && ctx.frame_index() == self.warmup_frames {
            ctx.metrics_mut().clear();
            self.shadow_gpu.clear();
            self.measured_from = Some((Instant::now(), ctx.frame_index()));
        }

        // The level editor takes the whole frame when it is open: its own camera, its own
        // input, and no simulation step. Freezing the world is the difference between an
        // editor and a game with a palette in the corner — an author placing a wall should
        // not have to place it around a pack of enemies walking through the room.
        //
        // Behind the feature, so a player's build has neither the branch nor the crate.
        #[cfg(feature = "editor")]
        if self.update_editor(ctx)? {
            return Ok(());
        }

        {
            engine_core::profile_scope!(profiler, "Input");
            // The menu sees the frame's input first, and then gameplay sees it too: the
            // world is shared, so a player with a settings screen open is still standing in
            // it and still has to be able to move out of the way.
            self.update_menu(ctx);
            // The collision demo has no combat, no targets and no resources; running the
            // class's interface there would take its digits away from the test stations.
            if !self.menu.is_open()
                && !self.scene.is_collision()
                && !self.scene.is_vfx()
                && !self.scene.is_audio()
            {
                self.update_firemage_ui(ctx);
            }
            self.update_targeting_decal();
            if self.scene.is_character() {
                self.update_animation_override(ctx);
            }
            if self.scene.is_vfx() {
                self.update_vfx_demo(ctx);
            }
            if self.scene.is_audio() {
                self.update_audio_demo(ctx);
            }

            // While a spell is being aimed both buttons are spoken for — left confirms
            // where it lands, right cancels — and the talent and character windows own the
            // pointer too. The character sheet especially: the left button *orbits the
            // camera*, so without this a drag across the panel steers the world instead of
            // carrying an item, with the cursor hidden and re-centred the whole way.
            // Cancelled rather than skipped: skipping also skips the *end* of a look, which
            // would leave the cursor hidden if a button came up on the frame one of these
            // opened.
            // Edit mode and the slot menu are pointer interfaces over the whole screen, so
            // they own the cursor for the same reason: a hidden pointer cannot drag a frame
            // or pick a spell.
            if self.targeting.is_active()
                || self.talent_menu.is_open()
                || self.character_menu.is_open()
                || self.hud_editor.is_active()
                || self.slot_menu.is_open()
            {
                self.cancel_mouse_look();
            } else {
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
                self.bindings.write_world_commands(
                    ctx.input(),
                    self.controller.yaw,
                    &mut self.commands,
                );
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
                    self.extractor
                        .note_damage(damage.target, time, damage.critical);
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
            // What the extractor needs that the simulation does not have: where the camera
            // is, for level-of-detail selection, and how much *real* time passed, for
            // animation. Real rather than simulated, so a character animates at the frame
            // rate while the simulation keeps its fixed tick.
            self.extractor.set_view(ViewParams {
                camera_position: self.camera.position(),
                delta_seconds: ctx.time().real_delta().as_secs_f32(),
                lod_bias: LOD_BIAS,
            });
            self.extractor.extract(&self.simulation);
            // Effects read the same event slice the numbers and the flash just read, and
            // then spawn into the same `RenderWorld` the extractor filled — so an explosion
            // is culled, batched and drawn with everything else rather than beside it.
            //
            // Real time, not simulation time: an effect is presentation and is expected to
            // move every frame, which is what stops a 60 Hz simulation from looking like
            // 60 FPS.
            let vfx_started = Instant::now();
            self.vfx.observe(&self.simulation);
            self.drive_vfx_stress(ctx.time().real_delta().as_secs_f32());
            self.vfx.update(
                &self.simulation,
                ctx.time().real_delta().as_secs_f32(),
                self.extractor.render_world_mut(),
            );
            self.renderer.set_particles(self.vfx.particles());
            self.vfx_ms = vfx_started.elapsed().as_secs_f64() * 1e3;
            // After extraction, because it spawns into the same `RenderWorld` and would
            // otherwise be drawing last frame's collision state beside this frame's
            // character.
            #[cfg(feature = "debug-overlay")]
            self.collision_debug
                .rebuild(self.extractor.render_world_mut(), &self.simulation);
            // After the collision view, so its spheres are the last thing spawned and the
            // debug spheres it would otherwise wrap are already in the world marked
            // `ShadowCasting::NONE` — which is what keeps the caster view from drawing a
            // bound around every collision box.
            #[cfg(feature = "debug-overlay")]
            self.shadow_debug.rebuild(self.extractor.render_world_mut());
            self.extract_ms = started.elapsed().as_secs_f64() * 1e3 - self.vfx_ms;
        }

        {
            engine_core::profile_scope!(profiler, "Camera");
            // The render delta, not the simulation step: the camera is presentation and is
            // expected to move every frame, which is what stops a 60 Hz simulation from
            // looking like 60 FPS.
            // The *interpolated* position, so the camera and the character it watches
            // agree about where the character is. Following the raw simulation position
            // would step the camera at 60 Hz behind a character moving smoothly.
            let position =
                engine_presentation::extract::interpolated_player_position(&self.simulation);
            // A teleport is not movement, so it is not smoothed: the camera arrives with the
            // character instead of chasing it across the gap at the follow rate.
            if engine_presentation::extract::player_teleported(&self.simulation) {
                self.controller.snap_to(position);
            } else {
                self.controller
                    .follow(position, ctx.time().real_delta().as_secs_f32());
            }
            let (width, height) = self.renderer.size();
            if width > 0 && height > 0 {
                self.controller
                    .apply(&mut self.camera, aspect_of(width, height))?;
            }
        }

        {
            engine_core::profile_scope!(profiler, "Audio");
            let started = Instant::now();
            // After the camera, because the listener *is* the camera: reading it here means
            // a sound is placed against the view the player is about to be shown rather
            // than against last frame's.
            //
            // One lock for the whole stage. The device's callback takes the same one for the
            // length of a mix; both are short, and the arrangement is documented in
            // `crate::audio_device`.
            if let Ok(mut audio) = self.audio.system().lock() {
                self.audio_director.set_listener(&self.camera, &mut audio);
                // The same event slice the numbers, the flash and the effects just read.
                // Nothing in the simulation knows any of the four exist.
                self.audio_director.observe(&self.simulation, &mut audio);
                self.audio_director.update(
                    &self.simulation,
                    ctx.time().real_delta().as_secs_f32(),
                    &mut audio,
                );
            }
            self.audio_ms = started.elapsed().as_secs_f64() * 1e3;
        }

        // After the camera, because it is a distance from the camera, and before the frame
        // is drawn. Nothing happens unless something crossed a threshold: for a still camera
        // this is one subtract and one compare per object over a few hundred objects, which
        // is why it is not on an interval.
        self.frame_seconds = ctx.time().real_delta().as_secs_f32();

        if self.environment.is_some() {
            engine_core::profile_scope!(profiler, "Environment");
            self.placer.update_lod(
                self.extractor.render_world_mut(),
                self.camera.position(),
                LOD_BIAS,
            );
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
        let first = self.renderer.frames_presented() == 0;
        if first {
            crate::startup::stage("frame 1: update");
        }
        self.renderer.render_world(
            self.extractor.render_world(),
            &self.camera,
            self.lighting,
            self.clear_color,
        )?;
        if first {
            crate::startup::stage("frame 1: record");
        }
        Ok(())
    }

    fn present(&mut self, _ctx: &mut EngineContext) -> Result<(), EngineError> {
        self.renderer.present()?;
        if self.renderer.frames_presented() == 1 {
            crate::startup::stage("frame 1: present");
            engine_renderer::log_pipeline_startup_counters();
            crate::startup::report();
        }
        // The window appears here, once, with a frame already in it. `--hidden` never takes
        // this branch, which is what makes it a run that renders without a window rather
        // than a run that does not render.
        if !self.window_shown && self.show_window {
            self.window.set_visible(true);
            self.window_shown = true;
            log::info!(
                "window shown after the first frame{}",
                if self.window.activates() {
                    ""
                } else {
                    ", without taking focus"
                }
            );
        }
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
        // The stage's timestamps are resolved a frame or two behind, so a frame with no
        // reading is skipped rather than recorded as zero.
        if let Some(milliseconds) = self.renderer.shadow_stats().gpu_ms {
            self.shadow_gpu.record_ms(milliseconds);
        }
        self.advance_benchmark(ctx);
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
        // The same last chance for the interface layout: a drag interrupted by a crash-free
        // quit, or a write that failed earlier.
        self.save_hud_layout();
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

    let ground_material: MaterialHandle =
        renderer.create_material(MaterialDesc::colored(0.16, 0.20, 0.17).with_roughness(0.95))?;
    let marker_material = renderer.create_material(
        MaterialDesc::colored(0.98, 0.86, 0.25)
            .with_roughness(0.30)
            .with_emissive(Vec3::new(0.5, 0.42, 0.05)),
    )?;

    // The two dummy models. `create_model` validates before it uploads, so a malformed
    // model fails here with the name of the model and the place inside it rather than
    // drawing something wrong.
    let dummy = renderer.create_model(&training_dummy(DummyVariant::Single))?;
    let dummy_group = renderer.create_model(&training_dummy(DummyVariant::Group))?;

    Ok(VisualBindings {
        dummy,
        dummy_group,
        ground_mesh,
        ground_bounds,
        ground_material,
        marker_mesh: cube_mesh,
        marker_bounds: cube_bounds,
        marker_material,
    })
}

/// The level `--scene-file` or `--layout` names, if either did.
///
/// Both are refused together rather than one silently winning: they are two authoring
/// routes to the same thing, and a command line that names both is a mistake worth stopping
/// for. A file that cannot be read or does not describe a level is a startup failure, for the
/// same reason a mis-exported model is — an authored level that silently falls back to the
/// built-in layout looks exactly like an authored level that did not save.
///
/// # Errors
///
/// [`EngineError`] naming the file and what is wrong inside it.
fn load_authored_scene(
    args: &Args,
) -> Result<Option<engine_environment::EnvironmentScene>, EngineError> {
    match (args.scene_file.as_deref(), args.layout.as_deref()) {
        (Some(_), Some(_)) => Err(EngineError::unsupported(
            "load both --scene-file and --layout; they are two ways to say what the level is",
        )),
        (Some(path), None) => {
            // A level that does not exist yet, opened in the editor, is a *new* level. Only
            // in the editor: a game told to load a level and quietly handed an empty one
            // instead would look exactly like a level that failed to save.
            if args.editor && !std::path::Path::new(path).is_file() {
                let name = std::path::Path::new(path).file_stem().map_or_else(
                    || "untitled".to_owned(),
                    |stem| stem.to_string_lossy().into_owned(),
                );
                log::info!(
                    "scene file: {path} does not exist yet - starting a new empty level '{name}'"
                );
                return Ok(Some(engine_environment::EnvironmentScene::empty(name)));
            }
            let scene = engine_environment::scene_file::load(path).map_err(|error| {
                EngineError::unsupported("load the scene file").with_source(error)
            })?;
            log::info!(
                "scene file: '{}' from {path} - {} objects, {} of them blocking",
                scene.name,
                scene.len(),
                scene.blocking_count()
            );
            Ok(Some(scene))
        }
        (None, Some(path)) => {
            let (scene, report) =
                engine_assets::disk::load_layout("layout", std::path::Path::new(path)).map_err(
                    |error| EngineError::unsupported("load the layout export").with_source(error),
                )?;
            log::info!(
                "layout: {path} - {} objects placed, {} nodes named nothing in the library",
                report.placed,
                report.skipped
            );
            Ok(Some(scene))
        }
        (None, None) => Ok(None),
    }
}

/// Makes sure a model is uploaded for every asset the scene names.
///
/// ```text
///  "pillar"                                  -> the compiled library
///  "generated/environment/rocks/rock_small"  -> assets/<id>.glb, through the importer
///  neither                                   -> the grey "missing asset" marker
/// ```
///
/// The library is uploaded at startup because a level made of it is the common case and one
/// pass is cheaper than eighty lookups. Everything else arrives here: a level that names a
/// file, or an author placing one in the editor while the game is running. Each asset is
/// uploaded once and then shares its handles with every placement of it, exactly as a library
/// asset does — the instancing story is unchanged.
///
/// A file that will not load is reported once and replaced by the marker, because an object
/// that is simply not drawn is indistinguishable from one nobody placed. Returns how many
/// models were uploaded.
fn upload_missing_models(
    renderer: &mut Renderer,
    visuals: &mut engine_presentation::EnvironmentVisuals,
    scene: &engine_environment::EnvironmentScene,
) -> usize {
    let mut uploaded = 0;
    for asset in scene.distinct_assets() {
        if visuals.get(asset).is_some() {
            continue;
        }
        let model = match environment_model_for(asset) {
            Some(model) => model,
            None => continue,
        };
        match renderer.create_model(&model) {
            Ok(binding) => {
                visuals.insert(asset, binding);
                uploaded += 1;
            }
            Err(error) => {
                log::error!("environment: '{asset}' could not be uploaded: {error}");
            }
        }
    }
    uploaded
}

/// The model an asset id names, or `None` if even the marker could not be built.
///
/// An id with a separator in it is a *path*: the catalogue's ids are the file's path under
/// the assets root with the extension taken off, so the file is found by putting one back.
/// Anything else is a name in the compiled library, and `environment_model` answers an
/// unknown one with the marker itself.
fn environment_model_for(
    asset: engine_environment::EnvironmentAsset,
) -> Option<engine_assets::ModelAsset> {
    let name = asset.name();
    if !name.contains('/') {
        return Some(engine_assets::procedural::environment_model(asset));
    }
    let limits = engine_assets::ValidationLimits::default();
    for extension in ["glb", "gltf"] {
        let Some(path) = engine_assets::disk::model_file(&format!("{name}.{extension}")) else {
            continue;
        };
        match engine_assets::disk::load_glb(name, &path, &limits) {
            Ok(model) => return Some(model),
            Err(error) => {
                // A file that is there and will not load is a content bug worth shouting
                // about; the marker goes in its place so the level still shows where the
                // object stands.
                log::error!("environment: '{name}' did not load: {error}");
                break;
            }
        }
    }
    log::warn!("environment: nothing on disk answers to '{name}'; drawing the missing marker");
    Some(engine_assets::procedural::environment_model(asset))
}

/// Uploads every environment asset in the library, once.
///
/// **This is the whole of the instancing requirement.** Sixteen `create_model` calls,
/// whatever the scene's object count is; every placement of an asset then refers to the
/// `MeshHandle` and `MaterialHandle` this returned, the render list's grouping pass finds
/// them contiguous, and each `(mesh, material)` pair becomes one `DrawIndexedInstanced`.
/// Adding a thousand of the same rock adds a thousand transforms and no GPU resources.
///
/// `create_model` validates before it uploads, so a malformed asset fails here naming the
/// asset and the place inside it rather than drawing something wrong.
fn upload_environment(
    renderer: &mut Renderer,
) -> Result<engine_presentation::EnvironmentVisuals, EngineError> {
    let mut visuals = engine_presentation::EnvironmentVisuals::new();
    let mut triangles = 0;
    let mut texture_bytes = 0;
    // Startup timing: the library is *authored in code* and then uploaded, alternately, so
    // the two costs cannot be told apart by a stage boundary. Two accumulators, reported as
    // two rows of the same table.
    let mut authoring = Duration::ZERO;
    let mut uploading = Duration::ZERO;
    let mut mark = Instant::now();
    for (asset, model) in engine_assets::procedural::environment_library() {
        authoring += mark.elapsed();
        mark = Instant::now();
        triangles += model.triangle_count();
        texture_bytes += model.texture_bytes();
        visuals.insert(asset, renderer.create_model(&model)?);
        uploading += mark.elapsed();
        mark = Instant::now();
    }
    crate::startup::record("  env: authored in code", authoring);
    crate::startup::record("  env: create_model", uploading);
    log::info!(
        "environment library: {} models, {triangles} triangles at LOD0, {:.0} KiB of textures",
        visuals.len(),
        texture_bytes as f64 / 1024.0,
    );
    Ok(visuals)
}

/// Where the camera will be on the first frame, near enough for the initial level choice.
///
/// The camera controller has not run yet when the environment is placed, so this stands in
/// for it: a few metres behind and above the player spawn. Being wrong costs one frame of
/// slightly-off detail and nothing else, because [`EnvironmentPlacer::update_lod`] corrects
/// it before anything is drawn — but starting the whole level at LOD0 and correcting on the
/// first frame would mean respawning a few hundred render entities during the first frame,
/// which is exactly the hitch a loading screen is meant to hide.
fn controller_eye_guess(scene: &engine_environment::EnvironmentScene) -> Vec3 {
    scene.spawns.player + Vec3::new(0.0, 4.0, 8.0)
}

/// One row of the hero table: who, which file an artist would deliver, how to build the
/// procedural stand-in, and what the character's definition is.
type HeroSource = (
    Hero,
    &'static str,
    fn() -> Result<engine_assets::ModelAsset, engine_assets::AssetError>,
    fn() -> engine_assets::CharacterDef,
);

/// Every hero's uploaded model, in [`Hero::ALL`] order.
///
/// **This is the whole of "what the player looks like".** A hero names a model
/// ([`Hero::model_name`]); this is where that name became triangles. Gameplay is not
/// involved, the extractor is handed a controller and asks nothing about who is in it, and a
/// fourth playable character is a row in the table below.
struct HeroVisuals {
    entries: Vec<HeroVisual>,
}

/// One hero's definition and uploaded geometry, kept so a swap can rebuild its controller
/// without touching the GPU.
struct HeroVisual {
    hero: Hero,
    definition: engine_assets::CharacterDef,
    model: std::sync::Arc<engine_assets::ModelAsset>,
    binding: engine_render_data::ModelBinding,
}

impl HeroVisuals {
    /// Uploads all three heroes.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if a model fails to build, fails to import or fails to upload.
    fn upload(renderer: &mut Renderer) -> Result<Self, EngineError> {
        // Which hero is built from what. The one table: an artist's export is looked for
        // under the hero's own file name, and the procedural model stands in until one is
        // delivered.
        let sources: [HeroSource; 3] = [
            (
                Hero::FireMage,
                "firemage.glb",
                fire_mage,
                fire_mage_character,
            ),
            (Hero::Warrior, "warrior.glb", warrior, warrior_character),
            (Hero::Druid, "druid.glb", druid, druid_character),
        ];

        let mut entries = Vec::with_capacity(sources.len());
        // Startup timing: as in `upload_environment`, building and uploading alternate.
        let mut building = Duration::ZERO;
        let mut uploading = Duration::ZERO;
        for (hero, file, build, character) in sources {
            let mark = Instant::now();
            let (definition, model) = load_character(hero, file, build, character)?;
            building += mark.elapsed();
            let mark = Instant::now();
            let binding = renderer.create_model(&model)?;
            uploading += mark.elapsed();
            entries.push(HeroVisual {
                hero,
                definition,
                model: std::sync::Arc::new(model),
                binding,
            });
        }
        crate::startup::record("  heroes: built/imported", building);
        crate::startup::record("  heroes: create_model", uploading);
        Ok(Self { entries })
    }

    /// A fresh visual controller for `hero`.
    ///
    /// A new controller rather than a stored one: a controller holds *where in which clip*
    /// the character is, and a hero that was mid-cast when it was taken off must not come
    /// back still holding that pose. The model and its upload are shared, so this allocates
    /// a bone palette and nothing else.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the model's clips cannot be bound, which validation at
    /// upload has already ruled out.
    fn visual(&self, hero: Hero) -> Result<CharacterVisual, EngineError> {
        let Some(entry) = self.entries.iter().find(|entry| entry.hero == hero) else {
            return Err(EngineError::unsupported("find the hero's model"));
        };
        CharacterVisual::new(
            entry.definition.clone(),
            std::sync::Arc::clone(&entry.model),
            entry.binding.clone(),
        )
        .map_err(|error| EngineError::unsupported("bind the hero's animations").with_source(error))
    }
}

/// One hero's model: the artist's export if one is installed, the procedural model if not.
///
/// An export that is present but does not load is an error rather than a silent fall back to
/// the procedural model: a mis-exported file and a file nobody delivered yet must not look
/// the same at startup.
fn load_character(
    hero: Hero,
    file: &str,
    build: fn() -> Result<engine_assets::ModelAsset, engine_assets::AssetError>,
    character: fn() -> engine_assets::CharacterDef,
) -> Result<(engine_assets::CharacterDef, engine_assets::ModelAsset), EngineError> {
    let name = hero.model_name();
    match engine_assets::disk::find_model(hero.config_name(), file) {
        Ok(Some(model)) => {
            log::info!(
                "{name}: imported {file} - {} triangles, {} bones, {} clips, {} KiB of \
                 textures, {:.2} m tall",
                model.triangle_count(),
                model
                    .skeleton
                    .as_ref()
                    .map_or(0, engine_assets::Skeleton::len),
                model.clips.len(),
                model.texture_bytes() / 1024,
                model.bind_pose_height(),
            );
            Ok((imported_character(&model, character()), model))
        }
        Ok(None) => Ok((
            character(),
            build().map_err(|error| {
                EngineError::unsupported("build a hero's model").with_source(error)
            })?,
        )),
        Err(error) => Err(EngineError::unsupported("import a hero's model").with_source(error)),
    }
}

/// `definition` retargeted at an imported model.
///
/// Two things a file decides and a definition cannot: how tall the model is authored — read
/// from the bind pose, so a rig authored in centimetres is still scaled to the capsule
/// gameplay collides with — and which of the definition's clip names the file actually
/// carries. A name the export does not have is dropped rather than refused, because a model
/// delivered before its animations should stand in its bind pose, not fail to load.
fn imported_character(
    model: &engine_assets::ModelAsset,
    definition: engine_assets::CharacterDef,
) -> engine_assets::CharacterDef {
    let mut definition = definition;
    let height = model.bind_pose_height();
    if height > 1e-3 {
        definition.model_height = height;
    }
    let before = definition.clips.len();
    definition
        .clips
        .retain(|(_, name)| model.clip_index(name).is_some());
    if definition.clips.len() < before {
        log::warn!(
            "{}: the export carries {} of the {before} clips the character wants; the rest \
             fall back to the bind pose",
            definition.name,
            definition.clips.len(),
        );
    }
    definition
}

/// The entities the ground indicator is drawn with: one per piece of each band.
///
/// Two pools rather than two entities. The indicator is an *outline* laid on the terrain, cut
/// into short pieces so each can sit at the height under it, and a pool is what lets the piece
/// count change with the shape and the radius without respawning anything: a piece the current
/// shape does not need is hidden, not despawned, and is reused the moment it is.
#[derive(Debug, Default)]
struct TargetingDecal {
    /// The bright line around the footprint, one entity per piece.
    border: Vec<engine_render_data::RenderEntity>,
    /// The translucent band just inside it, one entity per piece.
    glow: Vec<engine_render_data::RenderEntity>,
}

/// The mesh and materials the ground indicator is drawn with.
///
/// One mesh for the whole indicator. Every piece of every band and every shape is the same
/// unit quad, turned and stretched by its transform - which is what collapses the opaque
/// border into a single instanced draw however many pieces it has.
#[derive(Debug, Clone, Copy)]
struct DecalVisuals {
    /// A unit quad lying in XZ, facing up. One piece of a band.
    quad_mesh: MeshHandle,
    /// The quad's local bounds, for the cull.
    quad_bounds: engine_core::math::Sphere,
    /// The border of a valid placement: opaque, so it reads on any terrain material.
    border_valid: MaterialHandle,
    /// The border of a placement that would be refused.
    border_invalid: MaterialHandle,
    /// The translucent band inside a valid placement.
    glow_valid: MaterialHandle,
    /// The translucent band inside a refused one.
    glow_invalid: MaterialHandle,
}

/// Points `pool` at `transforms`, spawning, reusing and hiding entities as the count changes.
///
/// The piece count of the indicator changes with the ability - a cone is not a circle, and a
/// twelve-metre radius is not a five-metre one - and it changes while the level is being
/// edited under it. Despawning and respawning on every change would churn render-world slots
/// every frame a brush stroke moved the ground; a pool grows to the largest outline the
/// session has drawn and then never allocates again. Surplus entities are hidden rather than
/// removed, because the next shape is very likely to want them back.
fn sync_decal_band(
    world: &mut engine_render_data::RenderWorld,
    pool: &mut Vec<engine_render_data::RenderEntity>,
    transforms: &[engine_core::transform::Transform],
    mesh: MeshHandle,
    material: MaterialHandle,
    bounds: engine_core::math::Sphere,
) {
    for (entity, transform) in pool.iter().zip(transforms) {
        world.set_transform(*entity, *transform, bounds);
        world.set_material(*entity, material);
        world.set_visible(*entity, true);
    }
    for transform in transforms.iter().skip(pool.len()) {
        pool.push(world.spawn(engine_render_data::RenderEntityDesc {
            transform: *transform,
            mesh,
            material,
            bounds,
            visible: true,
            style: engine_render_data::InstanceStyle::NEUTRAL,
            // The targeting indicator: a shape painted on the ground to say where a spell
            // will land. It is interface, not matter - it casts nothing, and a shadow
            // falling across it must not make the area it marks harder to read.
            shadow: engine_render_data::ShadowCasting::NONE,
        }));
    }
    for entity in pool.iter().skip(transforms.len()) {
        world.set_visible(*entity, false);
    }
}

/// Uploads the ground indicator's geometry and colours.
///
/// **One mesh.** A unit quad, one metre across and one metre long, lying in XZ and facing up.
/// Every piece of the outline is an instance of it, so the shape of the indicator lives
/// entirely in the transforms `engine_presentation::ui::ground_decal` produces and a new
/// footprint needs no new geometry.
///
/// **Two surface classes, deliberately.** The border is opaque: it is a line the player has to
/// read against grass, rock and shadow alike, it needs no blending to do that, and an opaque
/// material is what lets every piece of it collapse into one instanced draw. The band inside
/// it is transparent, because its whole job is to mark an area *without* hiding the ground -
/// which is the one thing the opaque disc it replaces could not do.
fn upload_decal_visuals(renderer: &mut Renderer) -> Result<DecalVisuals, EngineError> {
    let quad = MeshData::plane(1.0);
    let quad_bounds = quad.bounds();
    let quad_mesh = renderer.create_mesh(&quad)?;

    // **Colour in the emissive term, base colour near black.** The shader's lit and ambient
    // contributions both scale with the base colour, so a band that carried its colour there
    // would shade differently on every piece as the pieces tilted to the ground - a ring of
    // facets on a slope - and would dim in a shadow exactly where the terrain is hardest to
    // read. Emissive is added after the lighting, so the indicator is one flat colour
    // whatever the ground under it is doing. Verified in an offscreen capture over rough
    // ground, which is where the lit version faceted visibly.
    let border_valid = renderer.create_material(
        MaterialDesc::colored(0.02, 0.013, 0.005)
            .with_roughness(1.0)
            .with_emissive(Vec3::new(1.15, 0.60, 0.17)),
    )?;
    let border_invalid = renderer.create_material(
        MaterialDesc::colored(0.02, 0.005, 0.004)
            .with_roughness(1.0)
            .with_emissive(Vec3::new(1.05, 0.14, 0.10)),
    )?;
    // Low enough that the terrain reads through it plainly. The number is the difference
    // between "this area is marked" and "this area is covered": at this alpha the band adds
    // about a fifth of its own colour over whatever the ground is.
    let glow_valid = renderer.create_material(
        MaterialDesc::colored(0.01, 0.006, 0.002)
            .with_roughness(1.0)
            .with_emissive(Vec3::new(0.95, 0.48, 0.13))
            .with_opacity(0.22),
    )?;
    let glow_invalid = renderer.create_material(
        MaterialDesc::colored(0.01, 0.003, 0.002)
            .with_roughness(1.0)
            .with_emissive(Vec3::new(0.88, 0.12, 0.09))
            .with_opacity(0.22),
    )?;

    Ok(DecalVisuals {
        quad_mesh,
        quad_bounds,
        border_valid,
        border_invalid,
        glow_valid,
        glow_invalid,
    })
}

/// Uploads the six shapes every effect is drawn with, and the one material they share.
///
/// **One material for the whole VFX system.** Colour is per instance — `InstanceStyle`'s
/// tint and emissive — so two effects of different colours still share a batch, and the
/// render list's grouping collapses every effect in the frame into six draws at most.
///
/// The sizes here are the contract `engine_presentation::vfx::VfxVisuals` documents: every
/// mesh is authored at unit size, so the director's transform is the gameplay number and
/// nothing else. A wedge is built with the *gameplay* cone's half-angle, so a cone effect
/// cannot show a different spread from the one the executor collects.
fn upload_vfx_visuals(renderer: &mut Renderer) -> Result<VfxVisuals, EngineError> {
    use engine_vfx::VfxShape;

    let material = renderer.create_material(
        MaterialDesc::colored(1.0, 1.0, 1.0)
            .with_roughness(1.0)
            .with_emissive(Vec3::ZERO),
    )?;
    let mut visuals = VfxVisuals::new(material);

    let shapes: [(VfxShape, MeshData); 6] = [
        (VfxShape::Disc, MeshData::disc(1.0, 48)),
        (VfxShape::Ring, MeshData::ring(1.0, 0.06, 48)),
        (
            VfxShape::Cone,
            MeshData::sector(1.0, engine_game::sim::CONE_HALF_ANGLE, 32),
        ),
        (VfxShape::Strip, MeshData::strip()),
        (VfxShape::Shell, MeshData::uv_sphere(1.0, 16, 12)),
        (VfxShape::Column, MeshData::cube(1.0)),
    ];
    for (shape, data) in shapes {
        let bounds = data.bounds();
        let mesh = renderer.create_mesh(&data)?;
        visuals.bind(shape, mesh, bounds);
    }
    log::info!("vfx library: {} shapes, 1 material", visuals.len());
    Ok(visuals)
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
        None if args.scene.is_vfx() => {
            log::info!(
                "vfx demo: {VFX_SCENE_DUMMIES} dummies and one key per effect in the catalogue"
            );
            for (index, effect) in engine_vfx::VfxId::ALL.into_iter().enumerate() {
                let key = [
                    "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "Z", "X", "C",
                ]
                .get(index)
                .copied()
                .unwrap_or("-");
                log::info!("  {key}  {}", effect.name());
            }
        }
        None if args.scene.is_dungeon() => {
            log::info!(
                "the Ashen Reliquary: walk forward. {} packs across {} rooms, then the boss",
                engine_game::dungeon::PACK_COUNT - 1,
                engine_environment::dungeon::AREA_COUNT,
            );
            log::info!("nothing spawns: every pack is standing there already, waiting to notice");
        }
        None if args.scene.is_encounter() => {
            log::info!("encounter demo: walk forward to be noticed; BACKSPACE resets the fight");
        }
        None if args.scene.is_audio() => {
            log::info!(
                "audio demo: {AUDIO_SCENE_DUMMIES} dummies, one key per sound, {} sounds in the catalogue",
                engine_audio::SOUND_COUNT
            );
        }
        None if args.scene.is_line_of_sight() => {
            log::info!(
                "line-of-sight course: three domes, with a dummy on the near slope, the crest and the far slope of the two outer ones"
            );
            log::info!("  TAB through them and cast: the far slope should be refused");
            log::info!("  the middle dome carries nobody: it is there to walk round and look past");
            log::info!("  drop into a pit: from the bottom of one, the floor is what blocks you");
            log::info!("  F3 draws the ray, the blocker and the reason");
        }
        None if args.scene.is_environment() => {
            log::info!("environment demo: the combat arena plus one of every asset, and no fight")
        }
        None if args.scene.is_shadows() => {
            log::info!("shadow course: six bays on flat ground, one per way a shadow goes wrong");
            log::info!("  -Z, far left   one pillar alone: bias, acne and peter-panning");
            log::info!(
                "  -Z, far right  {} crates in a grid: caster culling and the shadow LOD",
                engine_environment::shadow_course::CRATE_GRID.pow(2)
            );
            log::info!("  -Z, middle     rocks receding to the horizon: walk down them and watch");
            log::info!("                 the cascades change hands. A seam is a bug, a change of");
            log::info!("                 resolution is not");
            log::info!("  +Z, left       leaf screens: the silhouette is the texture's alpha");
            log::info!("  +Z, middle     a colonnade under three spot lights, of which the budget");
            log::info!("                 grants two at Medium and three at High");
            log::info!("  +Z, right      four braziers asking for six shadow views each. Walk");
            log::info!("                 into the bay: local shadows reach 40 m, so from here");
            log::info!("                 none of them are granted one");
            log::info!(
                "  F4 cycles the quality, F7 cycles the debug views: cascade tint, shadow                  factor, the atlas, caster bounds, then the material - normal,                  roughness, metallic, occlusion"
            );
        }
        None => log::info!("training arena: 1 dummy plus a group of {}", args.dummies),
    }

    log::info!("controls:");
    log::info!("  W A S D          move, relative to the camera");
    log::info!("  SPACE            jump");
    if demo.is_some() {
        log::info!("  1 - 7            stand at a test station");
        log::info!("  R                back to the first station");
    } else if args.scene.is_vfx() {
        log::info!("  1 - 0 Z X C      play one effect from the catalogue, in front of you");
    } else if args.scene.is_audio() {
        log::info!("  1 - 0            select one of the first ten sounds");
        log::info!("  Z / X            previous / next sound in the catalogue");
        log::info!("  C                drop it 8 m in front of you; walk to hear it fall off");
        log::info!("  V                play it with no position at all");
        log::info!("  B                stop everything");
        log::info!("  N                eight at once, to hear the limiter decide");
    } else {
        log::info!("  1 - 0            the current hero's ability bar");
        log::info!("  F / G            the two buttons beside it, for a hero with more");
        log::info!("                   than ten abilities: the Druid's wild rush and");
        log::info!("                   astral power");
        log::info!("  R                the ultimate: last stand, tranquility");
        log::info!("  BACKSPACE        reset the dummies and the combat meter");
        log::info!("  4 5 R            place with the mouse: left click confirms, right cancels");
        log::info!("  TAB              cycle target: the next enemy in range, nearest first");
        log::info!("  LEFT CLICK       select an enemy or its nameplate; hold and drag to orbit");
        log::info!("  F1               target yourself (or click your own frame)");
        log::info!("  RIGHT CLICK      a slot: change its spell, bind a key, clear it");
    }
    log::info!(
        "  ESC              drop the target, then the menu: edit hud, settings, quit, resume"
    );
    log::info!("  LEFT MOUSE       click to select, hold to orbit; the character keeps facing");
    log::info!("  RIGHT MOUSE      hold to turn the camera and steer the character");
    log::info!("  MOUSE WHEEL      camera distance");
    log::info!("  F3               collision debug drawing on/off");
    log::info!("  F5               developer overlay on/off");
    #[cfg(feature = "editor")]
    log::info!("  F6               level editor: place, move and delete environment objects");
    if args.scene.is_encounter() || args.scene.is_dungeon() {
        log::info!("  BACKSPACE        put the fight back: enemies home and whole, you alive");
        log::info!("  G                interrupt the target's cast (developer control)");
    }
    log::info!("  F4               shadow quality: off, low, medium, high, ultra");
    log::info!(
        "  F7               debug views: the shadows, then the material - shading normal,          roughness, metallic, occlusion"
    );
    log::info!("  F8               audio on/off");
    log::info!("  F9               wireframe on/off");
}

/// Pushes the player's four volumes into the mixer.
///
/// The settings are authoritative and the mixer is a view of them: every path that changes a
/// volume — loading the file, dragging a slider — ends here, so there is one place where a
/// number in `settings.cfg` becomes a number the mix multiplies by.
fn apply_volumes(audio: &AudioHost, settings: engine_settings::GameSettings) {
    let Ok(mut system) = audio.system().lock() else {
        return;
    };
    let mixer = system.mixer_mut();
    mixer.set_master(settings.master_volume);
    mixer.set_category(engine_audio::AudioCategory::Sfx, settings.sfx_volume);
    mixer.set_category(engine_audio::AudioCategory::Ui, settings.ui_volume);
    mixer.set_category(
        engine_audio::AudioCategory::Ambient,
        settings.ambient_volume,
    );
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

    #[test]
    fn every_aimed_ability_asks_for_geometry_the_indicator_can_draw() {
        // An ability whose shape fell through to a circle of radius zero would open a
        // targeting state and draw nothing, which is exactly what a missing indicator looks
        // like. Both bands are checked, at the exact widths and lifts the app uses, because
        // the band that gets its inset wrong is the band that collapses to a dot.
        use engine_presentation::ui::ground_decal::{DecalScratch, GroundSurface, build_band};
        use engine_presentation::ui::targeting::TargetShape;

        let ground = GroundSurface::Plane(0.0);
        let mut scratch = DecalScratch::new();
        let mut pieces = Vec::new();

        // Every hero's, not only the mage's: the Warrior's cleave and the Druid's Starfall
        // are aimed too, and an indicator that only knew about fire would draw nothing for
        // either.
        for ability in Hero::ALL.into_iter().flat_map(Hero::abilities).copied() {
            if !ability.definition().target.is_aimed() {
                continue;
            }
            let shape = TargetShape::of(ability);
            assert!(shape.extent() > 0.0, "{ability:?} has nothing to draw");

            let border = DecalBand::edged(DECAL_BORDER_WIDTH, DECAL_BORDER_LIFT);
            let glow = DecalBand::edged(DECAL_GLOW_WIDTH, DECAL_GLOW_LIFT)
                .with_inset(DECAL_BORDER_WIDTH + DECAL_GLOW_WIDTH * 0.5);
            for (name, band) in [("border", border), ("glow", glow)] {
                build_band(
                    shape,
                    Vec3::ZERO,
                    0.0,
                    band,
                    &ground,
                    &mut scratch,
                    &mut pieces,
                );
                assert!(
                    pieces.len() >= 4,
                    "{ability:?}: the {name} band is {} pieces",
                    pieces.len(),
                );
                assert!(
                    pieces.len() <= engine_presentation::ui::ground_decal::MAX_SEGMENTS,
                    "{ability:?}: the {name} band is over budget at {} pieces",
                    pieces.len(),
                );
            }
        }
    }
}
