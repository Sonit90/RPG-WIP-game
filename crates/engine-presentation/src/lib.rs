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
pub mod camera;
/// The character visual controller: gameplay state in, a posed and placed character out.
pub mod character;
pub mod collision_debug;
pub mod collision_hud;
pub mod damage_numbers;
pub mod environment;
pub mod extract;
pub mod game_ui;
pub mod hud;
pub mod mouse_look;
pub mod nameplate;
pub mod snapshot;
/// The reusable UI framework and the game's menus.
pub mod ui;
/// Combat drawn: the layer that decides what an ability looks like.
pub mod vfx;

pub use audio::{AudioDirector, AudioDirectorStats, HEAVY_DAMAGE, STRIDE};
pub use camera::ThirdPersonCamera;
pub use collision_debug::{CollisionDebug, DebugBindings};
pub use collision_hud::CollisionHudModel;
pub use damage_numbers::{FloatingNumber, FloatingNumbers, MAX_FLOATING_NUMBERS};
pub use environment::{EnvironmentPlacer, EnvironmentStats, EnvironmentVisuals, scene_lighting};
pub use extract::{
    RenderExtractor, VisualBindings, VisualStats, interpolated_player_position, lerp_angle,
    lerp_position,
};
pub use game_ui::{GameUi, UiTheme};
pub use hud::{HudModel, write_lines};
pub use mouse_look::{LOOK_BUTTON, LookChange, MouseLook, RelativeMotion};
pub use nameplate::{
    MAX_NAMEPLATES, NAMEPLATE_MAX_DISTANCE, Nameplate, NameplateConfig, NameplateSet,
    NameplateStatus, project,
};
pub use snapshot::{
    AbilitySlotView, CastView, CombatView, NotificationView, PlayerView, SlotAvailability,
    StatusList, StatusView, TargetView, UiSnapshot,
};
pub use ui::{
    MenuAction, MenuScreen, MenuSystem, MenuTheme, PauseItem, SettingsItem, UiInput, UiScreen,
    VolumeChannel, Widget, WidgetEvent, WidgetState, volume_of,
};
pub use vfx::{ShapeBinding, VfxDirector, VfxDirectorStats, VfxVisuals};
