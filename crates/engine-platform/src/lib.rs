//! Windows platform layer: window creation, message pumping, window events, frame pacing.
//!
//! This crate knows nothing about rendering. It hands out an opaque `HWND` value
//! (see [`Window::raw_handle`]) that the renderer turns into a swap chain; it never
//! links D3D12 itself, and the renderer never creates windows.
//!
//! Scope: one window, resize and close handling, clean shutdown, and the translation of
//! Windows keyboard and mouse messages into `engine-core` input events. Virtual-key codes
//! and `WPARAM` bit twiddling stop at this boundary; the engine above sees only
//! [`engine_core::input`] types.
//!
//! [`FramePacer`] is here rather than in `engine-core` for the same reason the window is: an
//! accurate wait needs a Win32 high-resolution timer, and `engine-core` links no OS API.

pub mod config;
/// Monitors and their display modes.
pub mod display;
pub mod dpi;
pub mod error;
pub mod event;
pub mod memory;
/// Holding the frame loop to a frame rate.
pub mod pacing;
/// Secrets at rest (DPAPI, current user) and the clipboard.
pub mod secure_store;
pub mod window;

pub use config::{AppliedDisplay, WindowConfig, WindowMode};
pub use display::{MonitorInfo, ScreenRect, VideoMode, primary_monitor};
pub use error::PlatformError;
pub use event::{EventQueue, WindowEvent, key_from_virtual_code};
pub use memory::{ProcessMemory, process_memory};
pub use pacing::FramePacer;
pub use window::{CursorClip, Window};
