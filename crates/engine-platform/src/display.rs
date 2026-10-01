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

/// One mode a monitor can be driven at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VideoMode {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Refresh rate in hertz. Zero or one means "the hardware default".
    pub refresh_hz: u32,
}

/// A rectangle on the virtual desktop, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// One monitor, as Windows describes it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// The device name, `\\.\DISPLAY1`: how a mode switch names the monitor.
    pub device: String,
    /// Whether it is the primary monitor.
    pub primary: bool,
    /// Where it is on the virtual desktop, at its current mode.
    pub bounds: ScreenRect,
    /// The part of it not covered by the taskbar.
    pub work_area: ScreenRect,
    /// The mode the desktop is configured for: what borderless fullscreen uses and what an
    /// exclusive switch returns to.
    pub desktop: VideoMode,
    /// Every distinct mode it offers, largest first, highest refresh first within a size.
    pub modes: Vec<VideoMode>,
}

impl MonitorInfo {
    /// The offered sizes without their refresh rates, each once, largest first.
    #[must_use]
    pub fn resolutions(&self) -> Vec<(u32, u32)> {
        let mut sizes: Vec<(u32, u32)> = self
            .modes
            .iter()
            .map(|mode| (mode.width, mode.height))
            .collect();
        sizes.dedup();
        sizes
    }

    /// The best mode at exactly `width` x `height`: the desktop's refresh rate if that size
    /// offers it, otherwise the highest it has.
    #[must_use]
    pub fn mode_for(&self, width: u32, height: u32) -> Option<VideoMode> {
        let mut candidates = self
            .modes
            .iter()
            .copied()
            .filter(|mode| mode.width == width && mode.height == height);
        let first = candidates.clone().next()?;
        Some(
            candidates
                .find(|mode| mode.refresh_hz == self.desktop.refresh_hz)
                .unwrap_or(first),
        )
    }
}

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

    #[test]
    fn the_best_mode_prefers_the_desktop_refresh() {
        let monitor = MonitorInfo {
            device: "\\\\.\\DISPLAY9".to_owned(),
            primary: true,
            bounds: ScreenRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            work_area: ScreenRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1040,
            },
            desktop: VideoMode {
                width: 1920,
                height: 1080,
                refresh_hz: 60,
            },
            modes: vec![
                VideoMode {
                    width: 1920,
                    height: 1080,
                    refresh_hz: 144,
                },
                VideoMode {
                    width: 1920,
                    height: 1080,
                    refresh_hz: 60,
                },
                VideoMode {
                    width: 1280,
                    height: 720,
                    refresh_hz: 144,
                },
            ],
        };
        assert_eq!(monitor.mode_for(1920, 1080).map(|m| m.refresh_hz), Some(60));
        assert_eq!(monitor.mode_for(1280, 720).map(|m| m.refresh_hz), Some(144));
        assert_eq!(monitor.mode_for(800, 600), None);
        assert_eq!(monitor.resolutions(), vec![(1920, 1080), (1280, 720)]);
    }
}
