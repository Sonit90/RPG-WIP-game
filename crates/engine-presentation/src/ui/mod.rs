//! A small retained UI: widgets, screens, input routing and the game's menus.
//!
//! ```text
//! InputState
//!     |
//!     v
//! UiInput          one struct: a pointer with edges, plus escape
//!     |
//!     v
//! UiScreen<T>      a list of tagged widgets, hit-testing
//!     |
//!     v
//! Widget           Button / Checkbox / Slider: state, a rule, and a value
//!     |
//!     v
//! WidgetEvent      Activated / Toggled / ValueChanged / ValueCommitted
//!     |
//!     v
//! MenuAction       what the application should do about it
//! ```
//!
//! ## Retained, not immediate
//!
//! Widgets exist between frames and hold their own state. An immediate-mode UI would rebuild
//! the whole tree every frame, which is exactly the per-frame allocation this engine spends
//! its time avoiding.
//!
//! ## Pointer only
//!
//! The menu answers the mouse, and escape to back out. It has no focus and no keyboard
//! navigation: the game keeps running while a menu is open, so every key the interface could
//! have claimed is one the player is still using to play.
//!
//! ## What this module does not do
//!
//! It does not draw itself — [`paint`] does, from the same data. It does not touch gameplay,
//! settings files or the renderer: every outcome leaves as a value. That is what lets the
//! whole framework be tested with no window, no device and no simulation, and it is what
//! keeps `if current_screen == Settings` out of the renderer.
//!
//! ## Growing it
//!
//! A new screen is a new tag enum, a `UiScreen<T>`, a layout function and a match arm in
//! [`menu`]. A new widget kind is a struct with an `update`, a variant in
//! [`widget::Widget`], and an arm in [`paint`]. Neither touches anything already here.

/// Eight action bars: what is in each slot, which key presses it, and where it is drawn.
pub mod action_bar;
pub mod aura_row;
/// The character sheet: stats, thirteen equipment slots, and the bag.
pub mod character_menu;
/// The chat panel: a log, an input line behind Enter, and the slash commands.
pub mod chat;
pub mod dungeon_map;
pub mod flow;
/// The look every unit frame, the raid frames and the cast bar share with the nameplates.
pub mod frame_style;
/// The online front-end: sign-in, characters, dungeons, roles, queue, ready check, party.
pub mod frontend;
/// The minimap and the full-screen dungeon map, with its encounter sidebar.
/// The GRAPHICS screen: resolution, window mode, shadow quality and the keep-or-revert dialog.
pub mod graphics_menu;
/// The ground indicator's geometry: an outline laid onto the terrain surface.
pub mod ground_decal;
/// The ground indicator's interior: a terrain-conforming triangle mesh filling a shape.
pub mod ground_fill;
/// Edit HUD: the interface made draggable.
pub mod hud_edit;
/// Where every arrangeable interface element sits, at any resolution.
pub mod hud_layout;
pub mod input;
pub mod kit;
/// The loot toast and the prompt over a waiting chest.
pub mod loot_hud;
/// The mailbox: letters with items the bag had no room for, and a way to take them.
pub mod mailbox;
/// The dungeon map's design tokens: colour, type, spacing, radii, motion.
pub mod map_style;
pub mod menu;
/// The Monster Combat Playground's panels: the library, the test controls, the encounter.
pub mod monster_playground;
pub mod paint;
/// The raid frames: one filled, rounded cell per member.
pub mod raid_frame;
pub mod screen;
/// What a right click on an action bar slot opens.
pub mod slot_menu;
pub mod talent_menu;
pub mod targeting;
pub mod tooltip;
/// The player, target, target-of-target and party frames.
pub mod unit_frame;
pub mod widget;

pub use action_bar::{
    ActionBar, ActionBarSet, ActionBarSlot, KeyConflict, SlotAddress, bar_size, slot_at, slot_rect,
};
pub use character_menu::{
    CharacterMenu, CharacterMenuOutcome, CharacterMetrics, CharacterTheme, Drag,
    push_character_menu, quality_color,
};
pub use dungeon_map::{
    DungeonMap, DungeonMapInfo, MapEncounterView, MapMarkers, MapOutcome, MapTheme, MapUnit,
};
pub use ground_decal::{
    DecalBand, DecalScratch, GroundSurface, MAX_SEGMENTS, SEGMENT_LENGTH, build_band,
};
pub use ground_fill::{FillMesh, MAX_FILL_INDICES, MAX_FILL_VERTICES, build_fill};
pub use hud_edit::{Control as EditControl, EditOutcome, EditTarget, HudEditor};
pub use hud_layout::{HudElementId, HudLayout, MIN_VISIBLE_FRACTION, clamp_on_screen};
pub use input::UiInput;
pub use mailbox::{MailboxMenu, MailboxOutcome, push_mailbox};
pub use menu::{
    MenuAction, MenuScreen, MenuSystem, NameplateItem, PauseItem, SettingsItem, VolumeChannel,
    volume_of,
};
pub use paint::{MenuScratch, MenuTheme, push_menu};
pub use screen::UiScreen;
pub use slot_menu::{SlotMenu, SlotMenuMode, SlotMenuOutcome};
pub use talent_menu::{TalentMenu, TalentMenuOutcome, TalentTheme, push_talent_menu};
pub use targeting::{GroundTarget, GroundTargeting, TargetShape, TargetingOutcome};
pub use unit_frame::{FrameScratch, push_frames};
pub use widget::{Button, Checkbox, Slider, Widget, WidgetEvent, WidgetState};
