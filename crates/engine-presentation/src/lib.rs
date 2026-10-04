//! The seam between gameplay and rendering.
//!
//! ```text
//! engine-game            engine-presentation           engine-render-data
//! ------------           -------------------           ------------------
//! Simulation      -->    RenderExtractor        -->    RenderWorld
//! player body     -->    ThirdPersonCamera      -->    Camera
//! Simulation      -->    UiSnapshot             -->    game UI (plain data)
//! frame metrics   -->    hud::write_lines       -->    [String]
//! enemies, camera -->    NameplateSet           -->    screen-space plates
//! ```
//!
//! The game UI reads a [`snapshot::UiSnapshot`] rather than the simulation. The developer
//! overlay in [`hud`] reads measurements only — frame times, tick timings, arena usage —
//! and the collision readout in [`collision_hud`] still borrows `&Simulation` directly,
//! which is what a debug readout is allowed to do; nothing the player sees does.
//!
//! This crate exists because the two things it joins must not know about each other.
//! `engine-game` cannot name a `MeshHandle`; `engine-render-data` cannot name a `Health`.
//! Something has to decide that a dummy is a box of a certain size in a certain colour, and
//! that something is presentation.
//!
//! It links no D3D12 either — `engine-render-data` does not — so every rule about where a
//! character's mesh ends up, how the camera follows it, and what the overlay prints is unit
//! tested without a device, a window or an adapter.

/// Combat heard: the layer that decides what an ability sounds like.
pub mod audio;
pub mod aura;
pub mod camera;
/// The character visual controller: gameplay state in, a posed and placed character out.
pub mod character;
/// The character file and the roster, in both directions.
pub mod character_save;
pub mod collision_debug;
pub mod collision_hud;
pub mod damage_numbers;
pub mod environment;
pub mod extract;
pub mod game_ui;
pub mod hud;
/// The developer readout for targeted-ability line of sight.
pub mod los_debug;
pub mod monster_visual;
pub mod mouse_look;
pub mod nameplate;
/// Where nameplates end up once they are not allowed to overlap.
pub mod nameplate_layout;
pub mod shadow_debug;
pub mod snapshot;
/// The reusable UI framework and the game's menus.
pub mod ui;
/// Combat drawn: the layer that decides what an ability looks like.
pub mod vfx;

pub use audio::{AudioDirector, AudioDirectorStats, HEAVY_DAMAGE, STRIDE};
pub use camera::ThirdPersonCamera;
pub use character_save::{apply_config, config_of};
pub use collision_debug::{CollisionDebug, DebugBindings};
pub use collision_hud::CollisionHudModel;
pub use damage_numbers::{
    CombatTextFilter, CombatTextKind, FloatingNumber, FloatingNumbers, MAX_FLOATING_NUMBERS,
};
pub use environment::{
    EnvironmentPlacer, EnvironmentStats, EnvironmentVisuals, scene_light, scene_lighting,
};
pub use extract::{
    RemoteFrame, RenderExtractor, VisualBindings, VisualStats, interpolated_player_position,
    lerp_angle,
    lerp_position,
};
pub use game_ui::{GameUi, UiTheme};
pub use hud::{HudModel, write_lines};
pub use monster_visual::{MonsterModel, MonsterVisuals};
pub use mouse_look::{LOOK_BUTTON, LookChange, MouseLook, RelativeMotion};
pub use nameplate::{
    MAX_NAMEPLATES, NAMEPLATE_MAX_DISTANCE, Nameplate, NameplateConfig, NameplateSet, project,
};
pub use nameplate_layout::{LayoutConstraints, NameplateLayout, PlateBox};
pub use shadow_debug::ShadowCasterDebug;
pub use snapshot::{
    AbilitySlotView, CastView, CharacterView, CombatView, NotificationView, PartyMemberView,
    PlayerView, SlotAvailability, StatusList, StatusView, TargetView, UiSnapshot,
};
pub use ui::{
    CharacterMenu, CharacterMenuOutcome, CharacterTheme, MenuAction, MenuScreen, MenuSystem,
    MenuTheme, NameplateItem, PauseItem, SettingsItem, UiInput, UiScreen, VolumeChannel, Widget,
    WidgetEvent, WidgetState, volume_of,
};
pub use vfx::{ShapeBinding, VfxDirector, VfxDirectorStats, VfxVisuals};
