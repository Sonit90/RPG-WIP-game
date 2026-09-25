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

/// What each character wears and carries, and the file it lives in.
pub mod character;
/// What the player can change about how the game plays.
pub mod gameplay;
/// What the player can change about how the game looks.
pub mod graphics;
/// The interface layout the player arranged: action bars, unit frames and their file.
pub mod hud;
/// How the minimap turns, and which key opens the dungeon map.
pub mod map;
/// How nameplates are sized, placed and separated.
pub mod nameplate;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use character::{
    CHARACTER_FILE_NAME, CharacterConfig, CharacterConfigManager, EquippedEntry, HeroLoadout,
    InventoryEntry,
};
pub use gameplay::GameplaySettings;
pub use graphics::{GraphicsSettings, ShadowQuality};
pub use hud::{
    ACTION_BAR_SLOTS, ACTION_BARS, ActionBarConfig, BindingScope, HUD_FILE_NAME, HUD_OFFSET_LIMIT,
    HUD_SCALE_DEFAULT, HUD_SCALE_MAX, HUD_SCALE_MIN, HUD_SCALE_STEP, HeroBindingsConfig, HudAnchor,
    HudBarBindings, HudConfig, HudConfigManager, HudFrames, HudOrientation, HudPlacement,
    HudSlotConfig,
};
pub use map::{KeyName, MapSettings, MinimapOrientation, TOGGLE_MAP_KEY_DEFAULT};
pub use nameplate::{NameplateSetting, NameplateSettings};

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

/// Smallest interface the slider offers, as a multiple of its design size.
///
/// Three quarters is as small as the 5x7 font stays legible: below it the glyph scale rounds
/// down to one and a nameplate becomes unreadable rather than merely small.
pub const UI_SCALE_MIN: f32 = 0.75;

/// Largest interface the slider offers.
///
/// Twice the design size, which is what a 4K display wants at a normal viewing distance —
/// the HUD is laid out in physical pixels, so without this it is half the size there that it
/// is at 1080p. It is also the largest scale the icon art is rasterized for; raising it
/// means rasterizing bigger icons, not just changing this number.
pub const UI_SCALE_MAX: f32 = 2.00;

/// The interface size a fresh install runs at.
///
/// One means "exactly the size the interface was designed at", so the default settings file
/// changes nothing about how the game already looks.
pub const UI_SCALE_DEFAULT: f32 = 1.00;

/// Granularity of the interface size slider.
///
/// A twentieth, so the slider stops on the round numbers a player names — 0.75, 1.0, 1.25,
/// 1.5, 2.0 — rather than between them.
pub const UI_SCALE_STEP: f32 = 0.05;

/// Quietest a volume slider goes: silence.
pub const VOLUME_MIN: f32 = 0.00;

/// Loudest. One is "as loud as the mix was authored"; above it would be amplification, and
/// amplification belongs to a limiter rather than to a slider.
pub const VOLUME_MAX: f32 = 1.00;

/// Granularity of a volume slider.
///
/// A twentieth, so the handle stops on the round numbers a player names and can return to.
pub const VOLUME_STEP: f32 = 0.05;

/// How loud a fresh install is overall.
///
/// Not 1.0: a game that opens at full output on a machine whose system volume is already up
/// is the first impression nobody wants, and 0.8 leaves room to turn *up*.
pub const MASTER_VOLUME_DEFAULT: f32 = 0.80;

/// How loud combat is, relative to the master.
pub const SFX_VOLUME_DEFAULT: f32 = 1.00;

/// How loud the interface is. Below the world, because a button press is confirmation and
/// not an event.
pub const UI_VOLUME_DEFAULT: f32 = 0.70;

/// How loud the bed under the arena is. Well below everything, because it never stops.
pub const AMBIENT_VOLUME_DEFAULT: f32 = 0.50;

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
    /// How big the interface is drawn, as a multiple of its design size.
    pub ui_scale: f32,
    /// How loud everything is, before any category volume.
    pub master_volume: f32,
    /// How loud the world is: abilities, impacts, footsteps.
    pub sfx_volume: f32,
    /// How loud the interface is.
    pub ui_volume: f32,
    /// How loud the environment bed is.
    pub ambient_volume: f32,
    /// What the player can change about how the game plays.
    ///
    /// Its own section for the same reason the nameplates have one: `settings.cfg` is meant
    /// to be opened and read, and a flat list of every setting in the game stops being one.
    pub gameplay: GameplaySettings,
    /// How nameplates are sized, placed and separated.
    ///
    /// A nested object rather than six more flat fields: nameplates are a screenful of
    /// related settings, and `settings.cfg` is meant to be opened and read. `#[serde(default)]`
    /// on [`NameplateSettings`] is what makes a file written before this section existed load
    /// with every one of them at its default rather than failing to parse.
    pub nameplates: NameplateSettings,
    /// What the player can change about how the game looks: shadow quality, so far.
    ///
    /// Its own section for the same reason the two above have one, and `#[serde(default)]`
    /// on [`GraphicsSettings`] is what lets a file written before shadows existed keep
    /// everything in it.
    pub graphics: GraphicsSettings,
    /// Whether the action bars' spells and keys belong to the account or to the character.
    ///
    /// A scalar in this file rather than a field of `hud.cfg` because it is a setting the
    /// player changes on the settings screen, and because it decides which half of the
    /// layout file is read: a setting that chose how to read the file it lived in would have
    /// to be found before the file it is in could be understood.
    #[serde(default)]
    pub binding_scope: BindingScope,
    /// How the minimap turns and which key opens the dungeon map.
    ///
    /// Its own section, like the graphics: `"map": {"minimap_orientation": "north_up"}`.
    /// `#[serde(default)]` on [`MapSettings`] is what lets a file written before the map
    /// existed load with both at their defaults.
    pub map: MapSettings,
}

impl Default for GameSettings {
    /// The one authoritative set of defaults.
    fn default() -> Self {
        Self {
            camera_sensitivity: CAMERA_SENSITIVITY_DEFAULT,
            ui_scale: UI_SCALE_DEFAULT,
            master_volume: MASTER_VOLUME_DEFAULT,
            sfx_volume: SFX_VOLUME_DEFAULT,
            ui_volume: UI_VOLUME_DEFAULT,
            ambient_volume: AMBIENT_VOLUME_DEFAULT,
            gameplay: GameplaySettings::default(),
            nameplates: NameplateSettings::default(),
            graphics: GraphicsSettings::default(),
            binding_scope: BindingScope::default(),
            map: MapSettings::default(),
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
        if !is_in_range(settings.ui_scale, UI_SCALE_MIN, UI_SCALE_MAX) {
            log::warn!(
                "settings: ui_scale {} is outside {UI_SCALE_MIN}..{UI_SCALE_MAX}; using the \
                 default {UI_SCALE_DEFAULT}",
                settings.ui_scale
            );
            settings.ui_scale = UI_SCALE_DEFAULT;
        }
        // The four volumes share one rule, so they share one loop rather than four copies of
        // the same three lines: a volume is a finite number between silence and full, and a
        // value that is not becomes the default for *that* volume.
        for (name, value, default) in [
            (
                "master_volume",
                &mut settings.master_volume,
                MASTER_VOLUME_DEFAULT,
            ),
            ("sfx_volume", &mut settings.sfx_volume, SFX_VOLUME_DEFAULT),
            ("ui_volume", &mut settings.ui_volume, UI_VOLUME_DEFAULT),
            (
                "ambient_volume",
                &mut settings.ambient_volume,
                AMBIENT_VOLUME_DEFAULT,
            ),
        ] {
            if !is_in_range(*value, VOLUME_MIN, VOLUME_MAX) {
                log::warn!(
                    "settings: {name} {} is outside {VOLUME_MIN}..{VOLUME_MAX}; using the                      default {default}",
                    *value
                );
                *value = default;
            }
        }
        // Each section validates itself, field by field, so one nonsense number in it costs
        // that number rather than the whole section.
        settings.gameplay = settings.gameplay.validated();
        settings.nameplates = settings.nameplates.validated();
        settings.graphics = settings.graphics.validated();
        settings.map = settings.map.validated();
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

    /// This value clamped into the volume range and snapped to the step.
    ///
    /// What every volume slider produces; kept here so no widget owns the rule.
    #[must_use]
    pub fn snap_volume(value: f32) -> f32 {
        if !value.is_finite() {
            return VOLUME_MAX;
        }
        let steps = ((value - VOLUME_MIN) / VOLUME_STEP).round();
        let snapped = VOLUME_MIN + steps * VOLUME_STEP;
        snapped.clamp(VOLUME_MIN, VOLUME_MAX)
    }

    /// This value clamped into the interface-size range and snapped to the step.
    ///
    /// What the slider produces; kept here so the widget does not own the rule.
    #[must_use]
    pub fn snap_ui_scale(value: f32) -> f32 {
        if !value.is_finite() {
            return UI_SCALE_DEFAULT;
        }
        let steps = ((value - UI_SCALE_MIN) / UI_SCALE_STEP).round();
        let snapped = UI_SCALE_MIN + steps * UI_SCALE_STEP;
        snapped.clamp(UI_SCALE_MIN, UI_SCALE_MAX)
    }
}

/// Reads a number, and reads anything that is not one as "no value".
///
/// A hand-edited file with `"size": "big"` in it is a file with one wrong field, not a file
/// that cannot be read: strict deserialization would fail the whole document and the player
/// would silently lose every other setting in it. Anything unreadable becomes `NaN`, which
/// each section's `validated` then replaces with that field's default — the same outcome an
/// out-of-range number gets, through the same code.
///
/// Shared by every section that wants it, which is why it lives here rather than beside one
/// of them.
pub(crate) fn lenient<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_f64().map_or(f32::NAN, f64_to_f32))
}

/// A JSON number as an `f32`, or `NaN` if it does not fit in one.
fn f64_to_f32(value: f64) -> f32 {
    let narrowed = value as f32;
    if narrowed.is_finite() {
        narrowed
    } else {
        f32::NAN
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
    ///
    /// Every other setting is carried over rather than rebuilt. Spelling out the whole
    /// struct here would silently reset any field this setter does not name, which is a bug
    /// that only appears once a second setting exists — so it is written the safe way from
    /// the start.
    pub fn set_camera_sensitivity(&mut self, value: f32) {
        self.update(GameSettings {
            camera_sensitivity: GameSettings::snap_camera_sensitivity(value),
            ..self.settings
        });
    }

    /// Sets the master volume, snapping it to the slider's step and range.
    pub fn set_master_volume(&mut self, value: f32) {
        self.update(GameSettings {
            master_volume: GameSettings::snap_volume(value),
            ..self.settings
        });
    }

    /// Sets the world volume, snapping it to the slider's step and range.
    pub fn set_sfx_volume(&mut self, value: f32) {
        self.update(GameSettings {
            sfx_volume: GameSettings::snap_volume(value),
            ..self.settings
        });
    }

    /// Sets the interface volume, snapping it to the slider's step and range.
    pub fn set_ui_volume(&mut self, value: f32) {
        self.update(GameSettings {
            ui_volume: GameSettings::snap_volume(value),
            ..self.settings
        });
    }

    /// Sets the ambience volume, snapping it to the slider's step and range.
    pub fn set_ambient_volume(&mut self, value: f32) {
        self.update(GameSettings {
            ambient_volume: GameSettings::snap_volume(value),
            ..self.settings
        });
    }

    /// Sets the interface size, snapping it to the slider's step and range.
    pub fn set_ui_scale(&mut self, value: f32) {
        self.update(GameSettings {
            ui_scale: GameSettings::snap_ui_scale(value),
            ..self.settings
        });
    }

    /// Sets where the action bars' spells and keys are kept.
    ///
    /// Nothing is moved here: this records the player's answer, and the application decides
    /// what the bars now are — which is the same division every other setter follows.
    pub fn set_binding_scope(&mut self, scope: BindingScope) {
        self.update(GameSettings {
            binding_scope: scope,
            ..self.settings
        });
    }

    /// Sets one nameplate setting, snapping it to that slider's step and range.
    ///
    /// The whole section is carried over and one field replaced, for the same reason every
    /// other setter here carries the rest of the struct: a setter that rebuilt the section
    /// would reset the five settings it does not name.
    pub fn set_nameplate(&mut self, setting: NameplateSetting, value: f32) {
        self.update(GameSettings {
            nameplates: self.settings.nameplates.with(setting, value),
            ..self.settings
        });
    }

    /// Sets the out-of-combat health regeneration rate, in percent of maximum per second.
    ///
    /// The simulation is told the result by the application; nothing here reaches gameplay.
    pub fn set_health_regen_percent(&mut self, value: f32) {
        self.update(GameSettings {
            gameplay: GameplaySettings {
                health_regen_percent_per_second: GameplaySettings::snap_health_regen_percent(value),
            },
            ..self.settings
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
            ui_scale: 1.25,
            master_volume: 0.55,
            sfx_volume: 0.9,
            ui_volume: 0.35,
            ambient_volume: 0.0,
            gameplay: GameplaySettings::default(),
            nameplates: NameplateSettings::default().with(NameplateSetting::Size, 14.0),
            graphics: GraphicsSettings {
                shadows: ShadowQuality::Ultra,
            },
            binding_scope: BindingScope::Character,
            map: MapSettings {
                minimap_orientation: MinimapOrientation::NorthUp,
                toggle_key: KeyName::new("N").unwrap_or_default(),
                reduced_motion: true,
            },
        };
        let text = serde_json::to_string(&settings).expect("settings serialize");
        let parsed: GameSettings = serde_json::from_str(&text).expect("settings parse");
        assert_eq!(parsed, settings);
        assert!(text.contains("camera_sensitivity"), "{text}");
        assert!(text.contains("master_volume"), "{text}");
        assert!(text.contains("\"binding_scope\":\"character\""), "{text}");
        assert!(
            text.contains("\"minimap_orientation\":\"north_up\""),
            "{text}"
        );
    }

    #[test]
    fn a_settings_file_written_before_the_binding_scope_existed_still_loads() {
        // The scope decides where a saved bar is read from, so a file that predates it must
        // load as the account-wide arrangement the game has always had rather than fail.
        let parsed: GameSettings =
            serde_json::from_str(r#"{"camera_sensitivity": 1.0}"#).expect("an old file parses");
        assert_eq!(parsed.binding_scope, BindingScope::Account);
    }

    #[test]
    fn the_binding_scope_is_set_without_disturbing_the_rest() {
        let dir = TempDir::new("binding-scope");
        let mut manager = SettingsManager::load(dir.file());
        manager.set_ui_scale(1.25);
        manager.set_binding_scope(BindingScope::Character);
        assert_eq!(manager.settings().binding_scope, BindingScope::Character);
        assert!((manager.settings().ui_scale - 1.25).abs() < f32::EPSILON);
        assert!(manager.save_if_dirty());

        let reloaded = SettingsManager::load(dir.file());
        assert_eq!(reloaded.settings().binding_scope, BindingScope::Character);
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
            ..GameSettings::default()
        };
        assert_eq!(
            settings.validated().camera_sensitivity,
            CAMERA_SENSITIVITY_DEFAULT
        );
        assert_eq!(
            GameSettings {
                camera_sensitivity: f32::INFINITY,
                ..GameSettings::default()
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
    fn the_interface_size_has_a_sane_range_around_its_default() {
        const { assert!(UI_SCALE_MIN < UI_SCALE_DEFAULT) };
        const { assert!(UI_SCALE_DEFAULT < UI_SCALE_MAX) };
        assert_eq!(
            UI_SCALE_DEFAULT, 1.0,
            "the default must change nothing about how the game already looks"
        );
    }

    #[test]
    fn an_out_of_range_or_meaningless_interface_size_falls_back_to_the_default() {
        for bad in [0.0, -1.0, 100.0, f32::NAN, f32::INFINITY] {
            let settings = GameSettings {
                ui_scale: bad,
                ..GameSettings::default()
            };
            assert_eq!(settings.validated().ui_scale, UI_SCALE_DEFAULT, "{bad}");
        }
    }

    #[test]
    fn the_interface_size_snaps_to_its_step_and_clamps_to_its_range() {
        assert_eq!(GameSettings::snap_ui_scale(1.26), 1.25);
        assert_eq!(GameSettings::snap_ui_scale(-5.0), UI_SCALE_MIN);
        assert_eq!(GameSettings::snap_ui_scale(99.0), UI_SCALE_MAX);
        assert_eq!(GameSettings::snap_ui_scale(f32::NAN), UI_SCALE_DEFAULT);

        // Every snapped value is one the loader will accept, or a dragged slider could
        // write a file that the next launch throws away.
        let mut value = UI_SCALE_MIN;
        while value <= UI_SCALE_MAX {
            let snapped = GameSettings::snap_ui_scale(value);
            let settings = GameSettings {
                ui_scale: snapped,
                ..GameSettings::default()
            };
            assert!(settings.is_valid(), "{snapped} would not survive a reload");
            value += UI_SCALE_STEP;
        }
    }

    #[test]
    fn setting_one_value_leaves_every_other_setting_alone() {
        // The bug a second setting makes possible: a setter that rebuilds the whole struct
        // resets everything it does not name, and the player loses an unrelated preference.
        let mut manager = SettingsManager::with_settings(
            "does-not-need-to-exist",
            GameSettings {
                camera_sensitivity: 3.0,
                ui_scale: 1.5,
                ..GameSettings::default()
            },
        );

        manager.set_camera_sensitivity(2.0);
        assert_eq!(manager.settings().ui_scale, 1.5, "the interface size reset");

        manager.set_ui_scale(1.25);
        assert_eq!(
            manager.settings().camera_sensitivity,
            2.0,
            "the sensitivity reset"
        );
        assert_eq!(manager.settings().ui_scale, 1.25);
    }

    #[test]
    fn updating_with_an_invalid_value_stores_the_default_instead() {
        let mut manager =
            SettingsManager::with_settings("does-not-need-to-exist", GameSettings::default());
        manager.update(GameSettings {
            camera_sensitivity: -1.0,
            ..GameSettings::default()
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
    fn a_settings_file_written_before_nameplates_existed_keeps_everything_in_it() {
        // The migration case: an existing player's file has no `nameplates` object at all.
        // It must load, keep the values it does have, and gain the defaults for the rest.
        let dir = TempDir::new("nameplates-migration");
        dir.write("{\"camera_sensitivity\": 2.5, \"ui_scale\": 1.25, \"master_volume\": 0.35}");

        let manager = SettingsManager::load(dir.file());
        let settings = manager.settings();
        assert_eq!(settings.camera_sensitivity, 2.5);
        assert_eq!(settings.ui_scale, 1.25);
        assert!((settings.master_volume - 0.35).abs() < 1e-6);
        assert_eq!(
            settings.nameplates,
            NameplateSettings::default(),
            "the missing section did not default"
        );

        // And the next launch reads back exactly what this one is running.
        let reloaded = SettingsManager::load(dir.file()).settings();
        assert_eq!(reloaded, settings);
    }

    #[test]
    fn a_file_written_before_the_gameplay_section_existed_gains_its_defaults() {
        let dir = TempDir::new("gameplay-migration");
        dir.write("{\"camera_sensitivity\": 2.0}");

        let settings = SettingsManager::load(dir.file()).settings();
        assert_eq!(settings.camera_sensitivity, 2.0);
        assert_eq!(settings.gameplay, GameplaySettings::default());
        assert!((settings.gameplay.health_regen_fraction() - 0.01).abs() < 1e-9);
    }

    #[test]
    fn the_regeneration_rate_survives_a_restart_and_an_invalid_one_does_not_break_the_file() {
        let dir = TempDir::new("gameplay-restart");
        {
            let mut first = SettingsManager::load(dir.file());
            first.set_health_regen_percent(2.5);
            first.save_if_dirty();
        }
        assert!(
            (SettingsManager::load(dir.file())
                .settings()
                .gameplay
                .health_regen_percent_per_second
                - 2.5)
                .abs()
                < 1e-6
        );

        dir.write(
            "{\"camera_sensitivity\": 1.5, \"gameplay\": {\"health_regen_percent_per_second\": -9}}",
        );
        let settings = SettingsManager::load(dir.file()).settings();
        assert_eq!(
            settings.camera_sensitivity, 1.5,
            "an unrelated setting reset"
        );
        assert_eq!(
            settings.gameplay.health_regen_percent_per_second,
            gameplay::HEALTH_REGEN_PERCENT_DEFAULT
        );
    }

    #[test]
    fn a_nameplate_setting_survives_a_restart_in_the_same_file() {
        let dir = TempDir::new("nameplates-restart");
        {
            let mut first = SettingsManager::load(dir.file());
            first.set_nameplate(NameplateSetting::Size, 16.0);
            first.set_nameplate(NameplateSetting::TopSafeZone, 0.2);
            assert!(first.is_dirty());
            first.save_if_dirty();
        }
        let second = SettingsManager::load(dir.file());
        assert_eq!(second.settings().nameplates.size, 16.0);
        assert!((second.settings().nameplates.top_safe_zone - 0.2).abs() < 1e-6);
        assert!(!second.is_dirty(), "a valid file was rewritten on load");

        // One file, not two: the nameplate settings live in the same `settings.cfg`.
        assert_eq!(
            second.path().file_name().and_then(std::ffi::OsStr::to_str),
            Some(SETTINGS_FILE_NAME)
        );
    }

    #[test]
    fn an_invalid_nameplate_value_in_the_file_becomes_that_settings_default() {
        let dir = TempDir::new("nameplates-invalid");
        dir.write(
            "{\"camera_sensitivity\": 1.5, \"nameplates\": {\"size\": -20, \
             \"vertical_margin\": 1.4, \"top_safe_zone\": \"soon\"}}",
        );

        let settings = SettingsManager::load(dir.file()).settings();
        assert_eq!(
            settings.camera_sensitivity, 1.5,
            "an unrelated setting reset"
        );
        assert_eq!(
            settings.nameplates.size,
            nameplate::NAMEPLATE_SIZE_DEFAULT,
            "an out-of-range size was kept"
        );
        assert_eq!(
            settings.nameplates.top_safe_zone,
            nameplate::NAMEPLATE_SAFE_ZONE_DEFAULT,
            "a value of the wrong type was kept"
        );
        assert!(
            (settings.nameplates.vertical_margin - 1.4).abs() < 1e-6,
            "a valid neighbour was thrown away with the invalid ones"
        );
    }

    #[test]
    fn changing_a_nameplate_setting_leaves_every_other_setting_alone() {
        let mut manager = SettingsManager::with_settings(
            "does-not-need-to-exist",
            GameSettings {
                camera_sensitivity: 3.0,
                ui_scale: 1.5,
                ..GameSettings::default()
            },
        );
        manager.set_nameplate(NameplateSetting::HeightAboveHead, 7.0);

        assert_eq!(manager.settings().camera_sensitivity, 3.0);
        assert_eq!(manager.settings().ui_scale, 1.5);
        assert_eq!(manager.settings().nameplates.height_above_head, 7.0);
        assert_eq!(
            manager.settings().nameplates.size,
            nameplate::NAMEPLATE_SIZE_DEFAULT,
            "a sibling nameplate setting reset"
        );
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
