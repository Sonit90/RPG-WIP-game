//! A small retained UI: widgets, screens, input routing and the game's menus.
//!
//! ```text
//! InputState
//!     |
//!     v
//! UiInput          one struct: a pointer with edges, and an optional navigation intent
//!     |
//!     v
//! UiScreen<T>      a list of tagged widgets, one focus, hit-testing
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
//! its time avoiding, and it would make "which widget has the focus" a thing to rediscover
//! sixty times a second rather than a field.
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
pub mod widget;

pub use input::{Nav, UiInput};
pub use menu::{MenuAction, MenuScreen, MenuSystem, PauseItem, SettingsItem};
pub use paint::{MenuScratch, MenuTheme, push_menu};
pub use screen::UiScreen;
pub use widget::{Button, Checkbox, Slider, Widget, WidgetEvent, WidgetState};
