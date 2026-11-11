//! Monitors and their display modes on macOS, from CoreGraphics.
//!
//! ```text
//! CGMainDisplayID / CGDisplayCopyDisplayMode ──> MonitorInfo { device, bounds, desktop mode }
//!                  CGDisplayCopyAllDisplayModes ┴──> every mode the display offers
//! ```
//!
//! CoreGraphics is used rather than winit's monitor list because [`primary_monitor`] has to
//! answer before any window, and so before any event loop, exists: the application sizes its
//! first window from it.
//!
//! Unlike Windows there is no mode switching here. A mode change on macOS would be an
//! exclusive-fullscreen display capture, which takes the display away from Mission Control,
//! the Dock and every other application and is slow to enter and to leave; the idiomatic
//! macOS game stays at the desktop's mode and fills the screen with a borderless window
//! (see [`crate::Window::apply_display`]). The modes are listed so the settings menu can
//! offer resolutions, and a window is sized to one of them in pixels.
//!
//! Sizes are physical pixels of the display's backing store, matching what the window
//! reports. On a Retina display the desktop mode is the pixel size (for example 3024x1964),
//! not the point size AppKit lays out in.

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyAllDisplayModes, CGDisplayCopyDisplayMode,
    CGDisplayMode, CGMainDisplayID,
};

pub use crate::monitor::{MonitorInfo, ScreenRect, VideoMode};

/// The primary monitor (the one with the menu bar): where a window that does not exist yet
/// will appear.
#[must_use]
pub fn primary_monitor() -> Option<MonitorInfo> {
    describe(CGMainDisplayID())
}

/// The monitor with CoreGraphics display id `id`, as winit's `native_id` names it.
pub(crate) fn monitor_for_display_id(id: CGDirectDisplayID) -> Option<MonitorInfo> {
    describe(id)
}

/// A mode as this crate describes it: pixel size and whole-hertz refresh rate.
fn video_mode(mode: &CGDisplayMode) -> VideoMode {
    // Zero is how CoreGraphics reports "no fixed rate" (the built-in ProMotion panel, most
    // LCDs on a variable-refresh link); the type documents zero as "the hardware default".
    let refresh = CGDisplayMode::refresh_rate(Some(mode));
    VideoMode {
        width: u32::try_from(CGDisplayMode::pixel_width(Some(mode))).unwrap_or(0),
        height: u32::try_from(CGDisplayMode::pixel_height(Some(mode))).unwrap_or(0),
        refresh_hz: if refresh.is_finite() && refresh > 0.0 {
            refresh.round() as u32
        } else {
            0
        },
    }
}

fn describe(display: CGDirectDisplayID) -> Option<MonitorInfo> {
    if display == 0 {
        return None;
    }
    let current = CGDisplayCopyDisplayMode(display)?;
    let desktop = video_mode(&current);
    if desktop.width == 0 || desktop.height == 0 {
        return None;
    }

    // CGDisplayBounds is in points on the global desktop; the origin is converted to pixels
    // with the ratio of this display's pixel size to its point size. For the primary display
    // the origin is (0, 0) and the ratio only matters to the size, which is the mode's own.
    let points = CGDisplayBounds(display);
    let scale = if points.size.width > 0.0 {
        f64::from(desktop.width) / points.size.width
    } else {
        1.0
    };
    let bounds = ScreenRect {
        x: (points.origin.x * scale).round() as i32,
        y: (points.origin.y * scale).round() as i32,
        width: desktop.width,
        height: desktop.height,
    };
    let primary = display == CGMainDisplayID();
    let work_area = if primary {
        work_area_of_primary(bounds, scale)
    } else {
        bounds
    };

    let mut modes = Vec::with_capacity(64);
    // SAFETY: `None` options is the documented way to ask for the default mode list.
    if let Some(all) = unsafe { CGDisplayCopyAllDisplayModes(display, None) } {
        for index in 0..all.count() {
            // SAFETY: `index` is below the array's count, and a CGDisplayMode array holds
            // CGDisplayMode references; the array outlives the borrow.
            let mode = unsafe { all.value_at_index(index) }.cast::<CGDisplayMode>();
            // SAFETY: a non-null element is a live CGDisplayMode owned by the array.
            let Some(mode) = (unsafe { mode.as_ref() }) else {
                continue;
            };
            if !CGDisplayMode::is_usable_for_desktop_gui(Some(mode)) {
                continue;
            }
            let found = video_mode(mode);
            if found.width > 0 && found.height > 0 {
                modes.push(found);
            }
        }
    }
    // The desktop mode is among the modes even if CoreGraphics filtered it from the list:
    // borderless fullscreen uses it, so the menu must be able to name it.
    if !modes.iter().any(|mode| {
        mode.width == desktop.width
            && mode.height == desktop.height
            && mode.refresh_hz == desktop.refresh_hz
    }) {
        modes.push(desktop);
    }
    modes.sort_by(|a, b| {
        (u64::from(b.width) * u64::from(b.height))
            .cmp(&(u64::from(a.width) * u64::from(a.height)))
            .then(b.width.cmp(&a.width))
            .then(b.refresh_hz.cmp(&a.refresh_hz))
    });
    modes.dedup();

    Some(MonitorInfo {
        device: format!("CGDisplay {display}"),
        primary,
        bounds,
        work_area,
        desktop,
        modes,
    })
}

/// The primary display minus the menu bar and the Dock, in pixels.
///
/// AppKit's `NSScreen` knows both; it is main-thread-only, so off the main thread (a unit
/// test, a worker) the whole display is returned rather than a guess.
fn work_area_of_primary(bounds: ScreenRect, scale: f64) -> ScreenRect {
    let Some(mtm) = MainThreadMarker::new() else {
        return bounds;
    };
    let Some(screen) = NSScreen::screens(mtm).firstObject() else {
        return bounds;
    };
    let frame = screen.frame();
    let visible = screen.visibleFrame();
    // Insets in points. AppKit's origin is bottom-left, so the menu bar is the gap above the
    // visible frame and the Dock the gap below (or at a side).
    let left = (visible.origin.x - frame.origin.x).max(0.0);
    let bottom = (visible.origin.y - frame.origin.y).max(0.0);
    let right =
        ((frame.origin.x + frame.size.width) - (visible.origin.x + visible.size.width)).max(0.0);
    let top =
        ((frame.origin.y + frame.size.height) - (visible.origin.y + visible.size.height)).max(0.0);
    let px = |points: f64| (points * scale).round() as i32;
    let width = bounds.width as i32 - px(left) - px(right);
    let height = bounds.height as i32 - px(top) - px(bottom);
    if width <= 0 || height <= 0 {
        return bounds;
    }
    ScreenRect {
        x: bounds.x + px(left),
        y: bounds.y + px(top),
        width: width as u32,
        height: height as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_primary_monitor_reports_its_desktop_mode_among_its_modes() {
        let Some(monitor) = primary_monitor() else {
            // A machine with no display at all (a CI session) has nothing to check.
            return;
        };
        assert!(monitor.primary);
        assert!(monitor.bounds.width > 0 && monitor.bounds.height > 0);
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
