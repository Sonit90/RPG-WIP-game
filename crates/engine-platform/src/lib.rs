//! Windows platform layer: window creation, message pumping, window events.
//!
//! This crate knows nothing about rendering. It hands out an opaque `HWND` value
//! (see [`Window::raw_handle`]) that the renderer turns into a swap chain; it never
//! links D3D12 itself, and the renderer never creates windows.
//!
//! Phase 0 / Milestone 1 scope: one window, resize and close handling, clean shutdown.
//! Input devices arrive with the gameplay layer, not here.

pub mod config;
pub mod dpi;
pub mod error;
pub mod event;
pub mod window;

pub use config::WindowConfig;
pub use error::PlatformError;
pub use event::{EventQueue, WindowEvent};
pub use window::Window;
