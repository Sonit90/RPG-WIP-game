//! Window configuration and its validation rules.
//!
//! This module is pure data: it contains no OS calls and is fully unit-testable.

use crate::error::PlatformError;

/// Smallest accepted client dimension, in pixels.
pub const MIN_DIMENSION: u32 = 64;

/// Largest accepted client dimension, in pixels.
///
/// Well above any current display, but low enough that `width * height * 16` bytes
/// cannot overflow a `u32` render-target size calculation downstream.
pub const MAX_DIMENSION: u32 = 16384;

/// How a window covers the screen.
///
/// The platform's own word for it: the settings crate has the player's, and the application
/// translates, so neither depends on the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowMode {
    /// A framed window, sized by its client area.
    #[default]
    Windowed,
    /// A borderless popup covering its monitor at the monitor's current mode.
    Borderless,
    /// A borderless popup covering its monitor, the monitor switched to a chosen mode.
    Fullscreen,
}

/// Where the cursor may go.
///
/// Mouse-look holds the cursor still: with raw input the cursor never needs to move for the
/// camera to turn, so it is pinned where the button went down and the pointer is exactly where
/// the player left it, ready to target. Without raw input the camera reads cursor positions,
/// so the cursor has to move and is only kept inside the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorClip {
    /// Anywhere on the desktop.
    #[default]
    Free,
    /// Inside the client area.
    Client,
    /// Held at one client-area point, in client pixels.
    Pinned(i32, i32),
}

/// What [`crate::Window::apply_display`] actually did: the mode and client size in force,
/// which a fallback can make different from what was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedDisplay {
    /// The mode in force.
    pub mode: WindowMode,
    /// Client width in pixels.
    pub width: u32,
    /// Client height in pixels.
    pub height: u32,
}

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
    /// Hidden windows are used by offscreen benchmarks, by tests that need a real `HWND`,
    /// and by an application that wants to draw its first frame before anything appears on
    /// screen. This is a normal runtime option, not a test hook.
    ///
    /// A hidden window still renders: the swap chain presents, the GPU does the frame's
    /// work, and `Present` returns `DXGI_STATUS_OCCLUDED`, which is a success code. That is
    /// what makes "headless, but really rendering" a configuration rather than a mode.
    pub visible: bool,
    /// Whether showing the window also gives it the keyboard focus.
    ///
    /// True is what a game does. False shows the window without activating it, so starting
    /// the game does not take the keyboard away from the editor it was started from — which
    /// is a development convenience and nothing else. The window can still be clicked into.
    pub activate: bool,
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
            activate: true,
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

    /// Returns the configuration with activation-on-show overridden.
    ///
    /// `false` shows the window without taking the keyboard focus.
    #[must_use]
    pub fn with_activate(mut self, activate: bool) -> Self {
        self.activate = activate;
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
            .with_resizable(false)
            .with_activate(false);
        assert!(!config.visible);
        assert!(!config.resizable);
        assert!(!config.activate);
        // And the default is a window a player would want: shown, and in front.
        let default = WindowConfig::default();
        assert!(default.visible);
        assert!(default.activate);
    }
}
