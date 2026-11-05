//! Platform layer: window creation, event pumping, window events, frame pacing.
//!
//! Windows (Win32) and macOS (winit over AppKit) are supported with an identical public API.
//! Win32 code lives beside this file, macOS code in `macos/`; the OS-independent parts
//! (configuration, the event queue and its coalescing, the pacing schedule, errors) are shared.
//!
//! This crate knows nothing about rendering. It hands out an opaque window handle (see
//! [`Window::raw_handle`]: an `HWND` on Windows, an `NSView` on macOS) that the renderer turns
//! into a swap chain or a Metal layer; it never links D3D12 or Metal itself, and the renderer
//! never creates windows.
//!
//! Scope: one window, resize and close handling, clean shutdown, and the translation of
//! native keyboard and mouse events into `engine-core` input events. Virtual-key codes
//! and `WPARAM` bit twiddling stop at this boundary; the engine above sees only
//! [`engine_core::input`] types.
//!
//! [`FramePacer`] is here rather than in `engine-core` for the same reason the window is: an
//! accurate wait needs OS timing (a Win32 high-resolution timer), and `engine-core` links no
//! OS API.

pub mod config;
/// Monitors and their display modes.
#[cfg(windows)]
pub mod display;
/// Monitors and their display modes.
#[cfg(target_os = "macos")]
#[path = "macos/display.rs"]
pub mod display;
#[cfg(windows)]
pub mod dpi;
#[cfg(target_os = "macos")]
#[path = "macos/dpi.rs"]
pub mod dpi;
pub mod error;
pub mod event;
pub mod memory;
mod monitor;
/// Holding the frame loop to a frame rate.
pub mod pacing;
/// Secrets at rest (DPAPI on Windows, the Keychain on macOS) and the clipboard.
#[cfg(windows)]
pub mod secure_store;
/// Secrets at rest (DPAPI on Windows, the Keychain on macOS) and the clipboard.
#[cfg(target_os = "macos")]
#[path = "macos/secure_store.rs"]
pub mod secure_store;
#[cfg(windows)]
pub mod window;
#[cfg(target_os = "macos")]
#[path = "macos/window.rs"]
pub mod window;

pub use config::{AppliedDisplay, CursorClip, WindowConfig, WindowMode};
pub use display::{MonitorInfo, ScreenRect, VideoMode, primary_monitor};
pub use error::PlatformError;
pub use event::{EventQueue, WindowEvent, key_from_virtual_code};
pub use memory::{ProcessMemory, process_memory};
pub use pacing::FramePacer;
pub use window::Window;
