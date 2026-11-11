//! Monitors and their display modes: which monitor the window is on, what it offers, and
//! switching it to a mode and back.
//!
//! ```text
//! MonitorFromWindow ──> GetMonitorInfoW ──> MonitorInfo { device, bounds, desktop mode }
//!                                              │
//!                         EnumDisplaySettingsW ┴──> every mode the monitor offers
//!
//! ChangeDisplaySettingsExW(device, mode, CDS_FULLSCREEN)   exclusive fullscreen
//! ChangeDisplaySettingsExW(device, null)                   back to the desktop's own
//! ```
//!
//! `CDS_FULLSCREEN` makes a switch temporary: Windows undoes it when the process ends, even
//! if the process ends by crashing, so a game that dies in fullscreen cannot leave the
//! desktop at 1280x720. Everything is per monitor — by device name — so a second monitor is
//! never touched by a switch meant for the first.
//!
//! Coordinates and sizes are physical pixels: the process is per-monitor DPI aware (see
//! [`crate::dpi`]) before any of this is asked.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    CDS_FULLSCREEN, CDS_TEST, CDS_TYPE, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE,
    DISP_CHANGE_SUCCESSFUL, DM_BITSPERPEL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH,
    ENUM_DISPLAY_SETTINGS_MODE, ENUM_REGISTRY_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW,
    HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MONITORINFOEXW,
    MonitorFromPoint, MonitorFromWindow,
};
use windows::core::PCWSTR;

use crate::error::PlatformError;

pub use crate::monitor::{MonitorInfo, ScreenRect, VideoMode};

/// The monitor holding most of `hwnd`, or the nearest one to it.
pub(crate) fn monitor_of_window(hwnd: HWND) -> Option<MonitorInfo> {
    // SAFETY: FFI call; a stale or null `hwnd` makes it return the nearest monitor or a null
    // handle, never undefined behaviour.
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    describe(monitor)
}

/// The primary monitor: where a window that does not exist yet will appear.
#[must_use]
pub fn primary_monitor() -> Option<MonitorInfo> {
    // SAFETY: FFI call; the origin is always on the primary monitor, and the flag makes it
    // return the primary even if it were not.
    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    describe(monitor)
}

fn describe(monitor: HMONITOR) -> Option<MonitorInfo> {
    if monitor.is_invalid() {
        return None;
    }
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` is a MONITORINFOEXW whose cbSize says so, which is how GetMonitorInfoW
    // knows it may also write the device name; the pointer is valid for the call.
    let found = unsafe { GetMonitorInfoW(monitor, (&raw mut info).cast::<MONITORINFO>()) };
    if !found.as_bool() {
        return None;
    }
    let device = &info.szDevice;
    let length = device
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(device.len());
    let terminated: Vec<u16> = device[..length]
        .iter()
        .copied()
        .chain(core::iter::once(0))
        .collect();
    let rect = |r: windows::Win32::Foundation::RECT| ScreenRect {
        x: r.left,
        y: r.top,
        width: (r.right - r.left).max(0) as u32,
        height: (r.bottom - r.top).max(0) as u32,
    };
    let bounds = rect(info.monitorInfo.rcMonitor);
    let desktop = registry_mode(&terminated).unwrap_or(VideoMode {
        width: bounds.width,
        height: bounds.height,
        refresh_hz: 0,
    });
    Some(MonitorInfo {
        device: String::from_utf16_lossy(&device[..length]),
        primary: info.monitorInfo.dwFlags & 1 != 0,
        bounds,
        work_area: rect(info.monitorInfo.rcWork),
        desktop,
        modes: modes(&terminated),
    })
}

/// A device name as the NUL-terminated wide string the display functions take.
fn wide(device: &str) -> Vec<u16> {
    device.encode_utf16().chain(core::iter::once(0)).collect()
}

/// The mode the desktop is configured for on `device` (NUL-terminated).
fn registry_mode(device: &[u16]) -> Option<VideoMode> {
    read_mode(device, ENUM_REGISTRY_SETTINGS)
}

fn read_mode(device: &[u16], which: ENUM_DISPLAY_SETTINGS_MODE) -> Option<VideoMode> {
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: `device` is NUL-terminated and outlives the call; `mode` is a DEVMODEW whose
    // dmSize is set, as the function requires.
    let ok = unsafe { EnumDisplaySettingsW(PCWSTR(device.as_ptr()), which, &mut mode) };
    ok.as_bool().then_some(VideoMode {
        width: mode.dmPelsWidth,
        height: mode.dmPelsHeight,
        refresh_hz: mode.dmDisplayFrequency,
    })
}

/// Every distinct mode `device` (NUL-terminated) offers at the desktop's colour depth or
/// deeper, largest first.
fn modes(device: &[u16]) -> Vec<VideoMode> {
    let mut found = Vec::with_capacity(64);
    // A real monitor lists a few hundred; the bound only stops a driver that never says
    // "no more" from hanging the menu.
    for index in 0..4096u32 {
        let mut mode = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        // SAFETY: as in `read_mode`; indices count up from zero until the call fails.
        let ok = unsafe {
            EnumDisplaySettingsW(
                PCWSTR(device.as_ptr()),
                ENUM_DISPLAY_SETTINGS_MODE(index),
                &mut mode,
            )
        };
        if !ok.as_bool() {
            break;
        }
        if mode.dmBitsPerPel < 24 || mode.dmPelsWidth == 0 || mode.dmPelsHeight == 0 {
            continue;
        }
        found.push(VideoMode {
            width: mode.dmPelsWidth,
            height: mode.dmPelsHeight,
            refresh_hz: mode.dmDisplayFrequency,
        });
    }
    found.sort_by(|a, b| {
        (u64::from(b.width) * u64::from(b.height))
            .cmp(&(u64::from(a.width) * u64::from(a.height)))
            .then(b.width.cmp(&a.width))
            .then(b.refresh_hz.cmp(&a.refresh_hz))
    });
    found.dedup();
    found
}

/// Switches `device` to `mode` until [`restore_mode`] or the end of the process.
///
/// Tested first (`CDS_TEST`), so a mode the driver would refuse is refused here, without the
/// screen going black on the way.
pub(crate) fn change_mode(device: &str, mode: VideoMode) -> Result<(), PlatformError> {
    let name = wide(device);
    let mut devmode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        dmPelsWidth: mode.width,
        dmPelsHeight: mode.height,
        dmBitsPerPel: 32,
        dmFields: DM_PELSWIDTH | DM_PELSHEIGHT | DM_BITSPERPEL,
        ..Default::default()
    };
    if mode.refresh_hz > 1 {
        devmode.dmDisplayFrequency = mode.refresh_hz;
        devmode.dmFields |= DM_DISPLAYFREQUENCY;
    }
    for flags in [CDS_TEST, CDS_FULLSCREEN] {
        // SAFETY: `name` is NUL-terminated and `devmode` is a fully initialized DEVMODEW;
        // both outlive the call. No window and no extra parameter are passed.
        let result = unsafe {
            ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&devmode), None, flags, None)
        };
        if result != DISP_CHANGE_SUCCESSFUL {
            return Err(PlatformError::InvalidConfig {
                reason: format!(
                    "{device} refused {}x{} at {} Hz ({})",
                    mode.width,
                    mode.height,
                    mode.refresh_hz,
                    describe_failure(result)
                ),
            });
        }
    }
    log::info!(
        "display: {device} switched to {}x{} at {} Hz",
        mode.width,
        mode.height,
        mode.refresh_hz
    );
    Ok(())
}

/// Puts `device` back to the mode the desktop is configured for.
pub(crate) fn restore_mode(device: &str) {
    let name = wide(device);
    // SAFETY: `name` is NUL-terminated and outlives the call; a null mode means "the
    // registry's settings", which is exactly the desktop's own mode.
    let result =
        unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), None, None, CDS_TYPE(0), None) };
    if result == DISP_CHANGE_SUCCESSFUL {
        log::info!("display: {device} back at its desktop mode");
    } else {
        log::warn!(
            "display: {device} could not be restored ({})",
            describe_failure(result)
        );
    }
}

fn describe_failure(result: DISP_CHANGE) -> &'static str {
    match result.0 {
        -1 => "the change failed",
        -2 => "the mode is not supported",
        -3 => "the registry could not be written",
        -4 => "the flags were wrong",
        -5 => "a parameter was wrong",
        -6 => "the driver needs a restart",
        1 => "the computer must restart",
        _ => "unknown reason",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_primary_monitor_reports_its_desktop_mode_among_its_modes() {
        let Some(monitor) = primary_monitor() else {
            // A machine with no monitor at all (a service session) has nothing to check.
            return;
        };
        assert!(monitor.primary);
        assert!(monitor.bounds.width > 0 && monitor.bounds.height > 0);
        assert!(monitor.device.starts_with("\\\\.\\"), "{}", monitor.device);
        if !monitor.modes.is_empty() {
            assert!(
                monitor
                    .mode_for(monitor.desktop.width, monitor.desktop.height)
                    .is_some(),
                "{:?} not among {:?}",
                monitor.desktop,
                monitor.resolutions()
            );
            let sizes = monitor.resolutions();
            let mut unique = sizes.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), sizes.len(), "each size once");
        }
    }
}
