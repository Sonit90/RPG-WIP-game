//! Player-facing settings: the values, their bounds, and the file they live in.
//!
//! ```text
//! GameSettings      values, bounds and the one definition of every default
//!      |
//!      v
//! SettingsManager   load, validate, hold, save
//!      |
//!      v
//! settings.cfg      JSON, beside the executable
//! ```
//!
//! Separate from [`engine_core::config::EngineSettings`](../engine_core/config) on purpose.
//! That type is engine wiring: parsed from the command line, validated once, never changed
//! again. This one is the player's: it changes while the game runs, it has to survive a
//! restart, and a garbage value in it must not stop the game from starting.
//!
//! ## Loading never fails
//!
//! [`SettingsManager::load`] returns settings, not a `Result`. A missing file, a malformed
//! file, a field of the wrong type, a field out of range and a field that is not there at
//! all all resolve to the same thing: that field's default. Anything else would mean a
//! hand-edited config could stop the game from launching, and the file exists to be
//! hand-edited.
//!
//! ## Bounds live with the value
//!
//! [`CAMERA_SENSITIVITY_MIN`] and friends are defined once, here. The slider reads them for
//! its range, the loader validates against them, and the camera scales its base rate by the
//! result. Nothing restates a default anywhere else — that is what makes "change the default
//! sensitivity" a one-line change rather than a search.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Name of the settings file. JSON content; the extension is deliberately not `.json`.
pub const SETTINGS_FILE_NAME: &str = "settings.cfg";

/// Slowest camera the slider offers, as a multiple of the camera's tuned rate.
pub const CAMERA_SENSITIVITY_MIN: f32 = 0.10;

/// Fastest camera the slider offers.
///
/// Five times the tuned rate is already faster than a player can follow; it exists so the
/// range covers a high-DPI mouse turned down in its own driver.
pub const CAMERA_SENSITIVITY_MAX: f32 = 5.00;

/// The camera sensitivity a fresh install runs at.
///
/// One means "exactly the rate the camera is tuned for", so the default settings file
/// changes nothing about how the game already plays.
pub const CAMERA_SENSITIVITY_DEFAULT: f32 = 1.00;

/// Granularity of the sensitivity slider.
///
/// Fine enough to tune, coarse enough that the number under the handle stops somewhere a
/// player can read and return to.
pub const CAMERA_SENSITIVITY_STEP: f32 = 0.05;

/// Everything the player can change.
///
/// Every field is a plain value with a default; nothing here knows about a widget, a file or
/// a camera. Adding a setting means a field, a bound, a line in [`GameSettings::validated`]
/// and a widget on a screen — and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameSettings {
    /// How fast the camera turns, as a multiple of its tuned rate.
    pub camera_sensitivity: f32,
}

impl Default for GameSettings {
    /// The one authoritative set of defaults.
    fn default() -> Self {
        Self {
            camera_sensitivity: CAMERA_SENSITIVITY_DEFAULT,
        }
    }
}

impl GameSettings {
    /// These settings with every out-of-range or non-finite field replaced by its default.
    ///
    /// Replaced rather than clamped: a value of `-100` is not a request for the minimum, it
    /// is a value that means nothing, and the honest answer to a meaningless setting is the
    /// default the player would have had without it.
    #[must_use]
    pub fn validated(self) -> Self {
        let mut settings = self;
        if !is_in_range(
            settings.camera_sensitivity,
            CAMERA_SENSITIVITY_MIN,
            CAMERA_SENSITIVITY_MAX,
        ) {
            log::warn!(
                "settings: camera_sensitivity {} is outside {CAMERA_SENSITIVITY_MIN}..\
                 {CAMERA_SENSITIVITY_MAX}; using the default {CAMERA_SENSITIVITY_DEFAULT}",
                settings.camera_sensitivity
            );
            settings.camera_sensitivity = CAMERA_SENSITIVITY_DEFAULT;
        }
        settings
    }

    /// Whether every field is finite and within its bounds.
    #[must_use]
    pub fn is_valid(self) -> bool {
        self == self.validated()
    }

    /// This value clamped into the sensitivity range and snapped to the step.
    ///
    /// What the slider produces; kept here so the widget does not own the rule.
    #[must_use]
    pub fn snap_camera_sensitivity(value: f32) -> f32 {
        if !value.is_finite() {
            return CAMERA_SENSITIVITY_DEFAULT;
        }
        let steps = ((value - CAMERA_SENSITIVITY_MIN) / CAMERA_SENSITIVITY_STEP).round();
        let snapped = CAMERA_SENSITIVITY_MIN + steps * CAMERA_SENSITIVITY_STEP;
        snapped.clamp(CAMERA_SENSITIVITY_MIN, CAMERA_SENSITIVITY_MAX)
    }
}

/// Whether `value` is a real number inside `[min, max]`.
fn is_in_range(value: f32, min: f32, max: f32) -> bool {
    value.is_finite() && value >= min && value <= max
}

/// Where the settings file lives, and what is currently in it.
///
/// Holds the settings in memory and writes them only when asked, which is what keeps a
/// dragged slider from writing to disk sixty times a second.
#[derive(Debug, Clone)]
pub struct SettingsManager {
    path: PathBuf,
    settings: GameSettings,
    /// Whether the in-memory settings differ from what was last written.
    dirty: bool,
}

impl SettingsManager {
    /// Loads the settings beside the running executable, creating the file if it is missing.
    ///
    /// Falls back to the working directory if the executable's own directory cannot be
    /// determined, which is a case that only arises on an unusual platform configuration.
    #[must_use]
    pub fn load_from_game_directory() -> Self {
        Self::load(game_directory().join(SETTINGS_FILE_NAME))
    }

    /// Loads `path`, repairing or creating it as needed. Never fails.
    ///
    /// The file is rewritten whenever what was read is not exactly what a valid file would
    /// contain — missing, malformed, or holding a value that had to be defaulted. That way
    /// the file on disk always describes the settings actually in use.
    #[must_use]
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (settings, needs_write) = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<GameSettings>(&text) {
                Ok(parsed) => {
                    let validated = parsed.validated();
                    (validated, validated != parsed)
                }
                Err(error) => {
                    log::warn!(
                        "settings: {} is not valid JSON ({error}); using defaults",
                        path.display()
                    );
                    (GameSettings::default(), true)
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                log::info!(
                    "settings: {} does not exist; writing the defaults",
                    path.display()
                );
                (GameSettings::default(), true)
            }
            Err(error) => {
                // Unreadable for some other reason: a permission problem, a directory in
                // its place. Play with the defaults rather than refusing to start, and do
                // not overwrite something that might be readable again later.
                log::warn!(
                    "settings: {} could not be read ({error}); using defaults",
                    path.display()
                );
                return Self {
                    path,
                    settings: GameSettings::default(),
                    dirty: false,
                };
            }
        };

        let mut manager = Self {
            path,
            settings,
            dirty: needs_write,
        };
        if needs_write {
            manager.save();
        }
        manager
    }

    /// A manager over `path` holding `settings`, with nothing read and nothing written.
    ///
    /// For tests and for a caller that has settings already.
    #[must_use]
    pub fn with_settings(path: impl Into<PathBuf>, settings: GameSettings) -> Self {
        Self {
            path: path.into(),
            settings: settings.validated(),
            dirty: false,
        }
    }

    /// The settings in use.
    #[must_use]
    pub fn settings(&self) -> GameSettings {
        self.settings
    }

    /// Where the settings are stored.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the in-memory settings have changes that are not on disk yet.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Replaces the settings in memory. Does not write anything.
    ///
    /// What a slider being dragged calls: the value is live immediately, and the disk is
    /// left alone until the interaction finishes.
    pub fn update(&mut self, settings: GameSettings) {
        let settings = settings.validated();
        if settings != self.settings {
            self.settings = settings;
            self.dirty = true;
        }
    }

    /// Sets the camera sensitivity, snapping it to the slider's step and range.
    pub fn set_camera_sensitivity(&mut self, value: f32) {
        self.update(GameSettings {
            camera_sensitivity: GameSettings::snap_camera_sensitivity(value),
        });
    }

    /// Writes the settings if they have changed since the last write.
    ///
    /// Returns whether anything was written. What a released slider handle calls.
    pub fn save_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.save();
        true
    }

    /// Writes the settings, whether or not they have changed.
    ///
    /// A failed write is logged and otherwise ignored: an unwritable config directory is a
    /// reason to lose a preference, not to lose the session.
    pub fn save(&mut self) {
        match write_settings(&self.path, self.settings) {
            Ok(()) => {
                self.dirty = false;
                log::debug!("settings: wrote {}", self.path.display());
            }
            Err(error) => log::warn!(
                "settings: could not write {} ({error}); the change is in memory only",
                self.path.display()
            ),
        }
    }
}

/// Serializes `settings` into `path`, creating the parent directory if needed.
fn write_settings(path: &Path, settings: GameSettings) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    // Pretty, with a trailing newline: the file is meant to be opened and edited by hand.
    let mut text = serde_json::to_string_pretty(&settings)?;
    text.push('\n');
    fs::write(path, text)
}

/// The directory the game runs from: the executable's own, or the working directory.
#[must_use]
pub fn game_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// A directory of this test's own, removed when the guard is dropped.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("engine-settings-{name}"));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a writable temporary directory");
            Self { path }
        }

        fn file(&self) -> PathBuf {
            self.path.join(SETTINGS_FILE_NAME)
        }

        fn write(&self, contents: &str) {
            fs::write(self.file(), contents).expect("a writable temporary file");
        }

        fn read(&self) -> String {
            fs::read_to_string(self.file()).expect("the settings file exists")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn the_defaults_are_valid_and_within_their_own_bounds() {
        let defaults = GameSettings::default();
        assert!(defaults.is_valid());
        assert_eq!(defaults.camera_sensitivity, CAMERA_SENSITIVITY_DEFAULT);
        const {
            assert!(CAMERA_SENSITIVITY_MIN < CAMERA_SENSITIVITY_DEFAULT);
            assert!(CAMERA_SENSITIVITY_DEFAULT < CAMERA_SENSITIVITY_MAX);
            assert!(CAMERA_SENSITIVITY_STEP > 0.0);
        }
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = GameSettings {
            camera_sensitivity: 2.35,
        };
        let text = serde_json::to_string(&settings).expect("settings serialize");
        let parsed: GameSettings = serde_json::from_str(&text).expect("settings parse");
        assert_eq!(parsed, settings);
        assert!(text.contains("camera_sensitivity"), "{text}");
    }

    #[test]
    fn a_missing_file_is_created_with_the_defaults() {
        let dir = TempDir::new("missing");
        assert!(!dir.file().exists());

        let manager = SettingsManager::load(dir.file());
        assert_eq!(manager.settings(), GameSettings::default());
        assert!(dir.file().exists(), "the file was not created");

        // What was written must be readable back as exactly the same settings.
        let reloaded = SettingsManager::load(dir.file());
        assert_eq!(reloaded.settings(), GameSettings::default());
        assert!(!reloaded.is_dirty(), "a valid file was rewritten");
    }

    #[test]
    fn a_valid_file_is_loaded_as_written() {
        let dir = TempDir::new("valid");
        dir.write("{\"camera_sensitivity\": 2.5}");
        let manager = SettingsManager::load(dir.file());
        assert_eq!(manager.settings().camera_sensitivity, 2.5);
    }

    #[test]
    fn a_missing_field_falls_back_to_its_default() {
        let dir = TempDir::new("missing-field");
        dir.write("{}");
        let manager = SettingsManager::load(dir.file());
        assert_eq!(
            manager.settings().camera_sensitivity,
            CAMERA_SENSITIVITY_DEFAULT
        );
    }

    #[test]
    fn a_field_of_the_wrong_type_falls_back_to_its_default() {
        let dir = TempDir::new("wrong-type");
        dir.write("{\"camera_sensitivity\": \"abc\"}");
        let manager = SettingsManager::load(dir.file());
        assert_eq!(
            manager.settings().camera_sensitivity,
            CAMERA_SENSITIVITY_DEFAULT
        );
        // The file described settings the game is not using; it is repaired.
        assert!(dir.read().contains("camera_sensitivity"));
        assert_eq!(
            SettingsManager::load(dir.file()).settings(),
            GameSettings::default()
        );
    }

    #[test]
    fn a_value_below_the_minimum_falls_back_to_its_default() {
        let dir = TempDir::new("below");
        dir.write("{\"camera_sensitivity\": -100}");
        let manager = SettingsManager::load(dir.file());
        assert_eq!(
            manager.settings().camera_sensitivity,
            CAMERA_SENSITIVITY_DEFAULT
        );
    }

    #[test]
    fn a_value_above_the_maximum_falls_back_to_its_default() {
        let dir = TempDir::new("above");
        dir.write("{\"camera_sensitivity\": 1000}");
        assert_eq!(
            SettingsManager::load(dir.file())
                .settings()
                .camera_sensitivity,
            CAMERA_SENSITIVITY_DEFAULT
        );
    }

    #[test]
    fn a_non_finite_value_falls_back_to_its_default() {
        // JSON has no NaN literal, so this is the path a value can only reach in memory.
        let settings = GameSettings {
            camera_sensitivity: f32::NAN,
        };
        assert_eq!(
            settings.validated().camera_sensitivity,
            CAMERA_SENSITIVITY_DEFAULT
        );
        assert_eq!(
            GameSettings {
                camera_sensitivity: f32::INFINITY
            }
            .validated()
            .camera_sensitivity,
            CAMERA_SENSITIVITY_DEFAULT
        );
    }

    #[test]
    fn malformed_json_does_not_stop_the_game_and_the_file_is_repaired() {
        let dir = TempDir::new("malformed");
        dir.write("{ this is not json at all ");

        let manager = SettingsManager::load(dir.file());
        assert_eq!(manager.settings(), GameSettings::default());

        let repaired = dir.read();
        let parsed: GameSettings =
            serde_json::from_str(&repaired).expect("the repaired file is valid JSON");
        assert_eq!(parsed, GameSettings::default());
    }

    #[test]
    fn unknown_fields_are_ignored_and_known_ones_still_load() {
        // Forwards compatibility: a file written by a later build must still start today's.
        let dir = TempDir::new("unknown");
        dir.write(
            "{\"camera_sensitivity\": 1.2, \"future_setting\": true, \"nested\": {\"a\": 1}}",
        );

        let manager = SettingsManager::load(dir.file());
        assert!((manager.settings().camera_sensitivity - 1.2).abs() < 1e-6);
    }

    #[test]
    fn an_empty_file_is_treated_as_malformed_rather_than_as_no_settings() {
        let dir = TempDir::new("empty");
        dir.write("");
        assert_eq!(
            SettingsManager::load(dir.file()).settings(),
            GameSettings::default()
        );
    }

    #[test]
    fn a_change_is_held_in_memory_and_written_only_when_asked() {
        let dir = TempDir::new("save");
        let mut manager = SettingsManager::load(dir.file());
        assert!(!manager.is_dirty());

        manager.set_camera_sensitivity(2.0);
        assert_eq!(manager.settings().camera_sensitivity, 2.0);
        assert!(manager.is_dirty(), "the change is not pending");
        assert_eq!(
            SettingsManager::load(dir.file())
                .settings()
                .camera_sensitivity,
            CAMERA_SENSITIVITY_DEFAULT,
            "the file was written during the drag"
        );

        assert!(manager.save_if_dirty(), "nothing was written");
        assert!(!manager.is_dirty());
        assert!(!manager.save_if_dirty(), "an unchanged value was rewritten");

        assert_eq!(
            SettingsManager::load(dir.file())
                .settings()
                .camera_sensitivity,
            2.0,
            "the change did not survive"
        );
    }

    #[test]
    fn a_setting_survives_a_restart() {
        // Run 1 changes it, run 2 must see it. The whole point of the file.
        let dir = TempDir::new("restart");
        {
            let mut first = SettingsManager::load(dir.file());
            first.set_camera_sensitivity(3.25);
            first.save_if_dirty();
        }
        let second = SettingsManager::load(dir.file());
        assert_eq!(second.settings().camera_sensitivity, 3.25);
        assert!(!second.is_dirty(), "a valid file was rewritten on load");
    }

    #[test]
    fn setting_a_value_snaps_it_into_range_and_onto_the_step() {
        let dir = TempDir::new("snap");
        let mut manager = SettingsManager::load(dir.file());

        manager.set_camera_sensitivity(-5.0);
        assert_eq!(
            manager.settings().camera_sensitivity,
            CAMERA_SENSITIVITY_MIN
        );

        manager.set_camera_sensitivity(999.0);
        assert_eq!(
            manager.settings().camera_sensitivity,
            CAMERA_SENSITIVITY_MAX
        );

        manager.set_camera_sensitivity(1.23);
        let snapped = manager.settings().camera_sensitivity;
        let steps = (snapped - CAMERA_SENSITIVITY_MIN) / CAMERA_SENSITIVITY_STEP;
        assert!(
            (steps - steps.round()).abs() < 1e-3,
            "{snapped} is off-step"
        );
        assert!((snapped - 1.25).abs() < 1e-6, "{snapped}");
    }

    #[test]
    fn snapping_a_meaningless_value_gives_the_default_rather_than_a_nonsense_number() {
        assert_eq!(
            GameSettings::snap_camera_sensitivity(f32::NAN),
            CAMERA_SENSITIVITY_DEFAULT
        );
    }

    #[test]
    fn updating_with_an_invalid_value_stores_the_default_instead() {
        let mut manager =
            SettingsManager::with_settings("does-not-need-to-exist", GameSettings::default());
        manager.update(GameSettings {
            camera_sensitivity: -1.0,
        });
        assert_eq!(
            manager.settings().camera_sensitivity,
            CAMERA_SENSITIVITY_DEFAULT
        );
        assert!(
            !manager.is_dirty(),
            "storing the default counted as a change"
        );
    }

    #[test]
    fn the_written_file_is_pretty_printed_json_a_person_can_edit() {
        let dir = TempDir::new("pretty");
        let mut manager = SettingsManager::load(dir.file());
        manager.set_camera_sensitivity(1.5);
        manager.save();

        let text = dir.read();
        assert!(text.starts_with('{'), "{text}");
        assert!(text.contains('\n'), "the file is one line: {text}");
        assert!(text.ends_with('\n'), "no trailing newline: {text:?}");
        assert!(text.contains("\"camera_sensitivity\""), "{text}");
    }

    #[test]
    fn the_game_directory_is_where_the_executable_is() {
        let directory = game_directory();
        assert!(
            directory.is_absolute() || directory == Path::new("."),
            "{directory:?}"
        );
    }
}
