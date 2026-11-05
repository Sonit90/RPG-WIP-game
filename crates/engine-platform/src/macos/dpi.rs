//! Display scaling on macOS: nothing to opt into.
//!
//! Windows lies to a process that has not declared itself DPI aware (see the Windows
//! version of this module); macOS does not. An AppKit view always reports *points* and a
//! backing scale factor, and winit hands this crate the physical (backing) size directly, so
//! every size the platform layer reports is already in real pixels without any process-wide
//! switch having been thrown.

/// Marks the process per-monitor DPI aware. On macOS it already is, so this does nothing.
///
/// Kept so the application calls the same function on every platform before it creates a
/// window.
pub fn set_process_dpi_aware() {
    log::debug!("macOS reports physical pixels per window; no DPI opt-in needed");
}
