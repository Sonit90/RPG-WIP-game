//! Engine configuration: one typed struct, validated once at startup.
//!
//! ## Why a plain struct
//!
//! No file format, no key-value store, no reflection. A typed struct is checked by the
//! compiler, and every consumer takes the field it needs rather than a string lookup that
//! can fail at runtime.
//!
//! It is also the reason nothing parses configuration per frame: the values are read once
//! into `WindowConfig`, `RendererConfig` and `EngineConfig`, and the subsystems then own
//! their own copies.
//!
//! ## Where each field goes
//!
//! ```text
//! EngineSettings
//!   ├── width / height / fullscreen  -> engine-platform (window)
//!   ├── vsync / renderer_validation  -> engine-renderer
//!   ├── debug_overlay                -> engine-renderer (feature-gated)
//!   └── frame_budget / report        -> engine-core (loop)
//! ```
//!
//! `engine-core` cannot depend on the platform or renderer crates, so this type holds the
//! values and the executable distributes them. That keeps configuration in one place
//! without inverting the dependency graph.

use crate::error::EngineError;

/// Smallest accepted window dimension, in pixels.
pub const MIN_DIMENSION: u32 = 64;

/// Largest accepted window dimension, in pixels.
pub const MAX_DIMENSION: u32 = 16384;

/// How the window covers the screen.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// A normal window with a border and title bar.
    #[default]
    Windowed,
    /// A borderless window covering the whole monitor.
    ///
    /// Borderless rather than exclusive fullscreen: alt-tab is instant, the desktop
    /// compositor keeps working, and there is no mode switch to fail. The cost is that
    /// the compositor is in the present path, which `docs/performance.md` records.
    BorderlessFullscreen,
}

impl DisplayMode {
    /// Whether this mode covers the whole monitor.
    #[must_use]
    pub fn is_fullscreen(self) -> bool {
        matches!(self, Self::BorderlessFullscreen)
    }
}

/// Everything the engine can be configured with.
///
/// Defaults are the shipping defaults, not the development ones: 1080p windowed, no
/// vsync, no overlay. Validation layers follow the build, since they cost several times
/// the CPU frame time.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineSettings {
    /// Client-area width in pixels. Ignored in fullscreen, which uses the monitor size.
    pub width: u32,
    /// Client-area height in pixels. Ignored in fullscreen.
    pub height: u32,
    /// Windowed or borderless fullscreen.
    pub display_mode: DisplayMode,
    /// Whether presentation waits for vertical blank.
    ///
    /// Off by default: a vsync-locked swap chain measures the monitor, not the engine.
    pub vsync: bool,
    /// Whether the debug overlay starts visible. It can be toggled at runtime.
    pub debug_overlay: bool,
    /// Whether to enable the graphics API's validation layer.
    ///
    /// Follows the build by default. It is a correctness tool that costs several times the
    /// frame time, so a release build with it on is measuring the wrong thing.
    pub renderer_validation: bool,
    /// Stop after this many frames. `None` runs until the window closes.
    pub frame_budget: Option<u64>,
    /// Seconds between frame-time reports. Zero disables them.
    pub report_interval_seconds: f64,
    /// Window title.
    pub title: String,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            display_mode: DisplayMode::Windowed,
            vsync: false,
            debug_overlay: false,
            renderer_validation: cfg!(debug_assertions),
            frame_budget: None,
            report_interval_seconds: 1.0,
            title: "myGame".to_owned(),
        }
    }
}

impl EngineSettings {
    /// Checks that the settings describe something the engine can start.
    ///
    /// Called once, before anything is created. A configuration error should be a message
    /// at startup, not a failed Win32 call three layers down.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] in the `Configuration` category naming the offending
    /// field and its value.
    pub fn validate(&self) -> Result<(), EngineError> {
        if self.title.trim().is_empty() {
            return Err(EngineError::configuration("window title must not be empty"));
        }

        // Dimensions are checked even in fullscreen, where they are unused: a fullscreen
        // session that later switches to windowed would otherwise fail at the switch,
        // which is a worse moment to find out.
        for (name, value) in [("width", self.width), ("height", self.height)] {
            if !(MIN_DIMENSION..=MAX_DIMENSION).contains(&value) {
                return Err(EngineError::configuration(format!(
                    "{name} {value} is outside {MIN_DIMENSION}..={MAX_DIMENSION}"
                )));
            }
        }

        if !self.report_interval_seconds.is_finite() || self.report_interval_seconds < 0.0 {
            return Err(EngineError::configuration(format!(
                "report interval {} must be zero or a positive number of seconds",
                self.report_interval_seconds
            )));
        }

        if self.frame_budget == Some(0) {
            return Err(EngineError::configuration(
                "a frame budget of zero would exit before rendering; omit it instead",
            ));
        }

        Ok(())
    }

    /// Whether frame-time reports are enabled.
    #[must_use]
    pub fn reports_enabled(&self) -> bool {
        self.report_interval_seconds > 0.0
    }

    /// Returns the settings with the window size applied.
    #[must_use]
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Returns the settings with the display mode applied.
    #[must_use]
    pub fn with_display_mode(mut self, mode: DisplayMode) -> Self {
        self.display_mode = mode;
        self
    }

    /// Returns the settings with vsync applied.
    #[must_use]
    pub fn with_vsync(mut self, vsync: bool) -> Self {
        self.vsync = vsync;
        self
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::error::ErrorCategory;

    #[test]
    fn the_defaults_are_1080p_windowed_and_valid() {
        let settings = EngineSettings::default();
        assert_eq!((settings.width, settings.height), (1920, 1080));
        assert_eq!(settings.display_mode, DisplayMode::Windowed);
        assert!(
            !settings.vsync,
            "vsync measures the monitor, not the engine"
        );
        assert!(!settings.debug_overlay);
        assert!(settings.frame_budget.is_none());
        assert!(settings.reports_enabled());
        settings.validate().expect("the defaults must be valid");
    }

    #[test]
    fn validation_layers_follow_the_build() {
        assert_eq!(
            EngineSettings::default().renderer_validation,
            cfg!(debug_assertions),
            "a release build must not pay for validation by default"
        );
    }

    #[test]
    fn dimensions_are_checked_against_their_bounds() {
        let settings = EngineSettings::default();

        assert!(
            settings
                .clone()
                .with_size(MIN_DIMENSION, 720)
                .validate()
                .is_ok()
        );
        assert!(
            settings
                .clone()
                .with_size(MAX_DIMENSION, MAX_DIMENSION)
                .validate()
                .is_ok()
        );

        let too_small = settings.clone().with_size(MIN_DIMENSION - 1, 720);
        let error = too_small.validate().expect_err("63 pixels is too small");
        assert_eq!(error.category(), ErrorCategory::Configuration);
        assert!(error.to_string().contains("width 63"), "{error}");

        let too_large = settings.with_size(1280, MAX_DIMENSION + 1);
        assert!(
            too_large
                .validate()
                .expect_err("16385 pixels is too large")
                .to_string()
                .contains("height 16385")
        );
    }

    #[test]
    fn dimensions_are_checked_even_in_fullscreen() {
        // They are unused there, but a later switch to windowed must not be the moment the
        // configuration turns out to be wrong.
        let settings = EngineSettings::default()
            .with_size(0, 0)
            .with_display_mode(DisplayMode::BorderlessFullscreen);
        assert!(settings.validate().is_err());
    }

    #[test]
    fn an_empty_title_is_rejected() {
        let settings = EngineSettings {
            title: "   ".to_owned(),
            ..EngineSettings::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn a_negative_or_non_finite_report_interval_is_rejected() {
        let mut settings = EngineSettings {
            report_interval_seconds: -1.0,
            ..EngineSettings::default()
        };
        assert!(settings.validate().is_err());

        settings.report_interval_seconds = f64::NAN;
        assert!(settings.validate().is_err());

        settings.report_interval_seconds = 0.0;
        assert!(settings.validate().is_ok(), "zero disables reporting");
        assert!(!settings.reports_enabled());
    }

    #[test]
    fn a_zero_frame_budget_is_rejected_rather_than_silently_exiting() {
        let mut settings = EngineSettings {
            frame_budget: Some(0),
            ..EngineSettings::default()
        };
        let error = settings.validate().expect_err("zero frames is a mistake");
        assert!(error.to_string().contains("omit it instead"), "{error}");

        settings.frame_budget = Some(1);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn overrides_replace_the_defaults() {
        let settings = EngineSettings::default()
            .with_size(1280, 720)
            .with_display_mode(DisplayMode::BorderlessFullscreen)
            .with_vsync(true);

        assert_eq!((settings.width, settings.height), (1280, 720));
        assert!(settings.display_mode.is_fullscreen());
        assert!(settings.vsync);
        settings.validate().expect("the overrides must be valid");
    }

    #[test]
    fn only_fullscreen_reports_itself_as_fullscreen() {
        assert!(!DisplayMode::Windowed.is_fullscreen());
        assert!(DisplayMode::BorderlessFullscreen.is_fullscreen());
        assert_eq!(DisplayMode::default(), DisplayMode::Windowed);
    }
}
