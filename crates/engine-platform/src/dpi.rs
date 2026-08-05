//! Per-monitor DPI awareness.
//!
//! Without this, Windows lies to a scaled process: it reports logical pixels and then
//! bitmap-stretches the result. A renderer that believes it is drawing 1920x1080 while
//! the compositor rescales it produces both a blurry image and untrustworthy
//! performance numbers, so awareness is opted into before any window is created.

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

/// Marks the process per-monitor DPI aware (V2).
///
/// Must be called before creating any window, and normally exactly once: a second call
/// fails with `ERROR_ACCESS_DENIED` because the context is already set. Any failure is
/// logged and otherwise ignored — a DPI-unaware process still runs, just scaled.
pub fn set_process_dpi_aware() {
    // SAFETY: FFI call taking a well-known constant context handle. It only mutates
    // process-wide DPI state and has no memory-safety preconditions.
    match unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        Ok(()) => log::debug!("process is per-monitor DPI aware (v2)"),
        Err(error) => {
            log::warn!("SetProcessDpiAwarenessContext failed, continuing scaled: {error}")
        }
    }
}
