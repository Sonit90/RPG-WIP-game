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

pub mod input;
pub mod menu;
pub mod paint;
pub mod screen;
pub mod talent_menu;
pub mod targeting;
pub mod widget;

pub use input::UiInput;
pub use menu::{MenuAction, MenuScreen, MenuSystem, PauseItem, SettingsItem};
pub use paint::{MenuScratch, MenuTheme, push_menu};
pub use screen::UiScreen;
pub use talent_menu::{TalentMenu, TalentMenuOutcome, TalentTheme, push_talent_menu};
pub use targeting::{GroundTarget, GroundTargeting, TargetingOutcome};
pub use widget::{Button, Checkbox, Slider, Widget, WidgetEvent, WidgetState};
