//! Window configuration and its validation rules.
//!
//! This module is pure data: it contains no Win32 calls and is fully unit-testable.

use crate::error::PlatformError;

/// Smallest accepted client dimension, in pixels.
pub const MIN_DIMENSION: u32 = 64;

/// Largest accepted client dimension, in pixels.
///
/// Well above any current display, but low enough that `width * height * 16` bytes
/// cannot overflow a `u32` render-target size calculation downstream.
pub const MAX_DIMENSION: u32 = 16384;

/// Initial state of a platform window.
///
/// The dimensions describe the *client* area — the region the renderer draws into —
/// not the outer window including borders and title bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowConfig {
    /// Text shown in the title bar.
    pub title: String,
    /// Client-area width in pixels.
    pub width: u32,
    /// Client-area height in pixels.
    pub height: u32,
    /// Whether the user may resize the window.
    pub resizable: bool,
    /// Whether the window is shown on creation.
    ///
    /// Hidden windows are used by offscreen benchmarks and by tests that need a real
    /// `HWND` without stealing focus. This is a normal runtime option, not a test hook.
    pub visible: bool,
    /// Whether to create a borderless window covering the primary monitor.
    ///
    /// When set, `width` and `height` are ignored: the window takes the monitor's size.
    /// Borderless rather than exclusive fullscreen — see
    /// [`engine_core::config::DisplayMode`].
    pub fullscreen: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "myGame".to_owned(),
            width: 1920,
            height: 1080,
            resizable: true,
            visible: true,
            fullscreen: false,
        }
    }
}

impl WindowConfig {
    /// Creates a configuration with the given title and client size.
    #[must_use]
    pub fn new(title: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            title: title.into(),
            width,
            height,
            ..Self::default()
        }
    }

    /// Returns the configuration with visibility overridden.
    #[must_use]
    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// Returns the configuration with resizability overridden.
    #[must_use]
    pub fn with_resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Returns the configuration with borderless fullscreen overridden.
    #[must_use]
    pub fn with_fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    /// Checks that the configuration can produce a real window.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidConfig`] if a dimension is outside
    /// [`MIN_DIMENSION`]..=[`MAX_DIMENSION`], or if the title is empty.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.title.is_empty() {
            return Err(PlatformError::InvalidConfig {
                reason: "window title must not be empty".to_owned(),
            });
        }
        for (name, value) in [("width", self.width), ("height", self.height)] {
            if !(MIN_DIMENSION..=MAX_DIMENSION).contains(&value) {
                return Err(PlatformError::InvalidConfig {
                    reason: format!("{name} {value} is outside {MIN_DIMENSION}..={MAX_DIMENSION}"),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_1080p_and_valid() {
        let config = WindowConfig::default();
        assert_eq!((config.width, config.height), (1920, 1080));
        assert!(config.visible);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn accepts_dimensions_on_the_boundaries() {
        assert!(
            WindowConfig::new("t", MIN_DIMENSION, MIN_DIMENSION)
                .validate()
                .is_ok()
        );
        assert!(
            WindowConfig::new("t", MAX_DIMENSION, MAX_DIMENSION)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn rejects_dimensions_outside_the_boundaries() {
        assert!(
            WindowConfig::new("t", MIN_DIMENSION - 1, 720)
                .validate()
                .is_err()
        );
        assert!(
            WindowConfig::new("t", 1280, MAX_DIMENSION + 1)
                .validate()
                .is_err()
        );
        assert!(WindowConfig::new("t", 0, 0).validate().is_err());
    }

    #[test]
    fn rejects_empty_title() {
        assert!(WindowConfig::new("", 1280, 720).validate().is_err());
    }

    #[test]
    fn builders_override_defaults() {
        let config = WindowConfig::new("t", 800, 600)
            .with_visible(false)
            .with_resizable(false);
        assert!(!config.visible);
        assert!(!config.resizable);
    }
}
