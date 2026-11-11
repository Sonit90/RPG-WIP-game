//! Monitor descriptions shared by every platform: a monitor, its modes, and picking among them.
//!
//! Pure data and pure functions. Each platform's `display` module fills these in from its own
//! API (Win32 `EnumDisplaySettings`, CoreGraphics display modes) and re-exports them, so the
//! public paths are the same everywhere.

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

/// One monitor, as the OS describes it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// The device name, `\\.\DISPLAY1`: how a mode switch names the monitor.
    pub device: String,
    /// Whether it is the primary monitor.
    pub primary: bool,
    /// Where it is on the virtual desktop, at its current mode.
    pub bounds: ScreenRect,
    /// The part of it not covered by the taskbar (Windows) or menu bar and Dock (macOS).
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

#[cfg(test)]
mod tests {
    use super::*;

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
