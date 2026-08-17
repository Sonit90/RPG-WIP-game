//! Nameplate settings: how big a plate is, where it floats, and how hard it is separated.
//!
//! ```text
//! NameplateSettings          what the player set, in the units the screen prints
//!      |
//!      v
//! into_* conversions         metres, alpha and glyph scale, which is what the game uses
//!      |
//!      v
//! NameplateConfig            presentation's own struct, built per frame
//! ```
//!
//! # Displayed units are not internal units
//!
//! The settings screen prints the numbers the player recognises — a size of `11`, a height of
//! `4`, an opacity of `40`. None of those are what the engine works in: the plate is drawn at
//! a glyph scale, it floats a number of *metres* above a head, and a colour's alpha is a
//! fraction. The conversion lives here, in one place, so the screen and the renderer cannot
//! disagree about what a number meant — and so the defaults are chosen to convert to exactly
//! what the game already did before there was a setting for it.
//!
//! | setting | default | what the default converts to |
//! | --- | --- | --- |
//! | size | 11 | the plate text scale the HUD already used |
//! | height above head | 4 | 0.90 m, the offset the plate already floated at |
//! | non-target opacity | 40 | alpha 0.40 |
//!
//! # Margins are multiples, not pixels
//!
//! [`NameplateSettings::vertical_margin`] and [`NameplateSettings::horizontal_margin`] are
//! multiples of a plate's own height and width, and [`NameplateSettings::top_safe_zone`] is a
//! fraction of the viewport's height. A pixel value would mean one thing at 1080p and another
//! at 4K, and the layout would have to be re-tuned per display.

use engine_core::tuning::nameplates as tuning;
use serde::{Deserialize, Serialize};

use crate::lenient;

/// Smallest nameplate the slider offers.
///
/// Below this the 5x7 font's glyph scale rounds down to one and the plate stops being
/// readable rather than merely becoming small.
pub const NAMEPLATE_SIZE_MIN: f32 = tuning::SIZE_MIN;

/// Largest nameplate the slider offers.
pub const NAMEPLATE_SIZE_MAX: f32 = tuning::SIZE_MAX;

/// The nameplate size a fresh install runs at.
///
/// The reference value: a size of 11 converts to a scale of exactly one, so the default
/// settings file draws the plate at the size the interface was designed at.
pub const NAMEPLATE_SIZE_DEFAULT: f32 = tuning::SIZE_DEFAULT;

/// Granularity of the size slider. Whole numbers, as the screen prints them.
pub const NAMEPLATE_SIZE_STEP: f32 = tuning::SIZE_STEP;

/// Closest to the head a plate may be anchored.
pub const NAMEPLATE_HEIGHT_MIN: f32 = tuning::HEIGHT_MIN;

/// Furthest above the head a plate may be anchored.
///
/// Twenty units is 4.5 m, which is above the tallest thing in the roster by more than its own
/// height — past it the plate has stopped describing the monster it belongs to.
pub const NAMEPLATE_HEIGHT_MAX: f32 = tuning::HEIGHT_MAX;

/// How far above the head a plate floats on a fresh install.
pub const NAMEPLATE_HEIGHT_DEFAULT: f32 = tuning::HEIGHT_DEFAULT;

/// Granularity of the height slider.
pub const NAMEPLATE_HEIGHT_STEP: f32 = tuning::HEIGHT_STEP;

/// Metres one unit of [`NameplateSettings::height_above_head`] is worth.
///
/// Chosen so the default of 4 is the 0.90 m the plate already floated at, which is what makes
/// a default settings file change nothing about how the game looks.
pub const NAMEPLATE_HEIGHT_UNIT: f32 = tuning::HEIGHT_UNIT_METRES;

/// Most transparent a non-target plate goes, as a percentage. Fully invisible is deliberately
/// offered: a player who wants only their target's plate in a fight can have that.
pub const NAMEPLATE_OPACITY_MIN: f32 = tuning::OPACITY_MIN;

/// Fully opaque, as a percentage. The plate is drawn exactly as the target's is.
pub const NAMEPLATE_OPACITY_MAX: f32 = tuning::OPACITY_MAX;

/// How visible a non-target plate is during a fight on a fresh install.
pub const NAMEPLATE_OPACITY_DEFAULT: f32 = tuning::OPACITY_DEFAULT;

/// Granularity of the opacity slider, in percent.
pub const NAMEPLATE_OPACITY_STEP: f32 = tuning::OPACITY_STEP;

/// Least separation the layout enforces: plates may touch.
pub const NAMEPLATE_MARGIN_MIN: f32 = tuning::MARGIN_MIN;

/// Most separation the layout enforces, as a multiple of a plate's own size.
///
/// Three plate heights apart is already more than a crowded pull can satisfy; past it every
/// plate in a pack is at its displacement limit and the setting stops meaning anything.
pub const NAMEPLATE_MARGIN_MAX: f32 = tuning::MARGIN_MAX;

/// Minimum vertical gap between two plates, as a multiple of a plate's height.
pub const NAMEPLATE_VERTICAL_MARGIN_DEFAULT: f32 = tuning::VERTICAL_MARGIN_DEFAULT;

/// Minimum horizontal gap between two plates, as a multiple of a plate's width.
pub const NAMEPLATE_HORIZONTAL_MARGIN_DEFAULT: f32 = tuning::HORIZONTAL_MARGIN_DEFAULT;

/// Granularity of both margin sliders.
pub const NAMEPLATE_MARGIN_STEP: f32 = tuning::MARGIN_STEP;

/// Smallest top safe zone: none at all.
pub const NAMEPLATE_SAFE_ZONE_MIN: f32 = tuning::SAFE_ZONE_MIN;

/// Largest top safe zone, as a fraction of the viewport's height.
///
/// Half the screen. Past it the protected region is most of the space a plate could be in.
pub const NAMEPLATE_SAFE_ZONE_MAX: f32 = tuning::SAFE_ZONE_MAX;

/// How much of the top of the screen plates keep out of on a fresh install.
pub const NAMEPLATE_SAFE_ZONE_DEFAULT: f32 = tuning::SAFE_ZONE_DEFAULT;

/// Granularity of the safe-zone slider.
pub const NAMEPLATE_SAFE_ZONE_STEP: f32 = tuning::SAFE_ZONE_STEP;

/// Which nameplate setting a value belongs to.
///
/// One enum shared by the settings file, the menu and the application, for the same reason
/// [`crate::GameSettings`] has one definition of every bound: six near-identical sliders that
/// each carried their own action would be six places to forget one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameplateSetting {
    /// Base font and plate size.
    Size,
    /// Distance between a monster's head and its plate's anchor.
    HeightAboveHead,
    /// Opacity of a plate that is not the target, while the player is fighting.
    NonTargetOpacity,
    /// Minimum vertical separation between stacked plates.
    VerticalMargin,
    /// Minimum horizontal separation between stacked plates.
    HorizontalMargin,
    /// Fraction of the top of the screen plates stay out of.
    TopSafeZone,
}

impl NameplateSetting {
    /// Every setting, in the order the screen stacks them.
    pub const ALL: [Self; 6] = [
        Self::Size,
        Self::HeightAboveHead,
        Self::NonTargetOpacity,
        Self::VerticalMargin,
        Self::HorizontalMargin,
        Self::TopSafeZone,
    ];

    /// The label its slider prints.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Size => "SIZE",
            Self::HeightAboveHead => "HEIGHT ABOVE HEAD",
            Self::NonTargetOpacity => "OPACITY FOR NON TARGETS (IN COMBAT)",
            Self::VerticalMargin => "VERTICAL MARGIN",
            Self::HorizontalMargin => "HORIZONTAL MARGIN",
            Self::TopSafeZone => "TOP OF SCREEN SAFE ZONE",
        }
    }

    /// Its slider's smallest value.
    #[must_use]
    pub fn minimum(self) -> f32 {
        match self {
            Self::Size => NAMEPLATE_SIZE_MIN,
            Self::HeightAboveHead => NAMEPLATE_HEIGHT_MIN,
            Self::NonTargetOpacity => NAMEPLATE_OPACITY_MIN,
            Self::VerticalMargin | Self::HorizontalMargin => NAMEPLATE_MARGIN_MIN,
            Self::TopSafeZone => NAMEPLATE_SAFE_ZONE_MIN,
        }
    }

    /// Its slider's largest value.
    #[must_use]
    pub fn maximum(self) -> f32 {
        match self {
            Self::Size => NAMEPLATE_SIZE_MAX,
            Self::HeightAboveHead => NAMEPLATE_HEIGHT_MAX,
            Self::NonTargetOpacity => NAMEPLATE_OPACITY_MAX,
            Self::VerticalMargin | Self::HorizontalMargin => NAMEPLATE_MARGIN_MAX,
            Self::TopSafeZone => NAMEPLATE_SAFE_ZONE_MAX,
        }
    }

    /// Its slider's granularity.
    #[must_use]
    pub fn step(self) -> f32 {
        match self {
            Self::Size => NAMEPLATE_SIZE_STEP,
            Self::HeightAboveHead => NAMEPLATE_HEIGHT_STEP,
            Self::NonTargetOpacity => NAMEPLATE_OPACITY_STEP,
            Self::VerticalMargin | Self::HorizontalMargin => NAMEPLATE_MARGIN_STEP,
            Self::TopSafeZone => NAMEPLATE_SAFE_ZONE_STEP,
        }
    }

    /// The value a fresh install has.
    #[must_use]
    pub fn default_value(self) -> f32 {
        match self {
            Self::Size => NAMEPLATE_SIZE_DEFAULT,
            Self::HeightAboveHead => NAMEPLATE_HEIGHT_DEFAULT,
            Self::NonTargetOpacity => NAMEPLATE_OPACITY_DEFAULT,
            Self::VerticalMargin => NAMEPLATE_VERTICAL_MARGIN_DEFAULT,
            Self::HorizontalMargin => NAMEPLATE_HORIZONTAL_MARGIN_DEFAULT,
            Self::TopSafeZone => NAMEPLATE_SAFE_ZONE_DEFAULT,
        }
    }

    /// How many decimal places the screen prints it to.
    ///
    /// The reference layout prints `11`, `4` and `40` whole and the three normalised values
    /// to two places. A number printed to more places than its step can reach is a number a
    /// player cannot land on.
    #[must_use]
    pub fn decimals(self) -> usize {
        match self {
            Self::Size | Self::HeightAboveHead | Self::NonTargetOpacity => 0,
            Self::VerticalMargin | Self::HorizontalMargin | Self::TopSafeZone => 2,
        }
    }

    /// `value` clamped into this setting's range and snapped onto its step.
    ///
    /// A value that is not a number at all is this setting's default: the honest answer to a
    /// meaningless setting is the one the player would have had without it.
    #[must_use]
    pub fn snap(self, value: f32) -> f32 {
        if !value.is_finite() {
            return self.default_value();
        }
        let (minimum, maximum, step) = (self.minimum(), self.maximum(), self.step());
        let steps = ((value - minimum) / step).round();
        (minimum + steps * step).clamp(minimum, maximum)
    }
}

/// Everything the player can change about nameplates.
///
/// Stored in the units the settings screen prints, not the units the engine draws with —
/// see the module documentation. Serialized as its own object inside `settings.cfg`, so a
/// file written before nameplates existed loads with every one of these at its default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NameplateSettings {
    /// Base font and plate size. Eleven is the size the interface was designed at.
    #[serde(deserialize_with = "lenient")]
    pub size: f32,
    /// Distance between the monster's head and the plate's anchor, in height units.
    #[serde(deserialize_with = "lenient")]
    pub height_above_head: f32,
    /// Opacity of a non-target plate while the player is in combat, as a percentage.
    #[serde(deserialize_with = "lenient")]
    pub non_target_opacity_in_combat: f32,
    /// Minimum vertical gap between two plates, as a multiple of a plate's height.
    #[serde(deserialize_with = "lenient")]
    pub vertical_margin: f32,
    /// Minimum horizontal gap between two plates, as a multiple of a plate's width.
    #[serde(deserialize_with = "lenient")]
    pub horizontal_margin: f32,
    /// Fraction of the top of the screen plates stay out of.
    #[serde(deserialize_with = "lenient")]
    pub top_safe_zone: f32,
}

impl Default for NameplateSettings {
    fn default() -> Self {
        Self {
            size: NAMEPLATE_SIZE_DEFAULT,
            height_above_head: NAMEPLATE_HEIGHT_DEFAULT,
            non_target_opacity_in_combat: NAMEPLATE_OPACITY_DEFAULT,
            vertical_margin: NAMEPLATE_VERTICAL_MARGIN_DEFAULT,
            horizontal_margin: NAMEPLATE_HORIZONTAL_MARGIN_DEFAULT,
            top_safe_zone: NAMEPLATE_SAFE_ZONE_DEFAULT,
        }
    }
}

impl NameplateSettings {
    /// The value of one setting.
    #[must_use]
    pub fn get(&self, setting: NameplateSetting) -> f32 {
        match setting {
            NameplateSetting::Size => self.size,
            NameplateSetting::HeightAboveHead => self.height_above_head,
            NameplateSetting::NonTargetOpacity => self.non_target_opacity_in_combat,
            NameplateSetting::VerticalMargin => self.vertical_margin,
            NameplateSetting::HorizontalMargin => self.horizontal_margin,
            NameplateSetting::TopSafeZone => self.top_safe_zone,
        }
    }

    /// These settings with one value replaced, snapped to that setting's step and range.
    ///
    /// Every other field is carried over rather than rebuilt, which is what stops a setter
    /// from silently resetting the five settings it does not name.
    #[must_use]
    pub fn with(mut self, setting: NameplateSetting, value: f32) -> Self {
        let value = setting.snap(value);
        match setting {
            NameplateSetting::Size => self.size = value,
            NameplateSetting::HeightAboveHead => self.height_above_head = value,
            NameplateSetting::NonTargetOpacity => self.non_target_opacity_in_combat = value,
            NameplateSetting::VerticalMargin => self.vertical_margin = value,
            NameplateSetting::HorizontalMargin => self.horizontal_margin = value,
            NameplateSetting::TopSafeZone => self.top_safe_zone = value,
        }
        self
    }

    /// These settings with every out-of-range or non-finite field replaced by its default.
    ///
    /// Per field, deliberately: a file with one nonsense number keeps the other five.
    #[must_use]
    pub fn validated(self) -> Self {
        let mut settings = self;
        for setting in NameplateSetting::ALL {
            let value = settings.get(setting);
            if !value.is_finite() || value < setting.minimum() || value > setting.maximum() {
                log::warn!(
                    "settings: nameplate {:?} {value} is outside {}..{}; using the default {}",
                    setting,
                    setting.minimum(),
                    setting.maximum(),
                    setting.default_value()
                );
                settings = settings.set_exact(setting, setting.default_value());
            }
        }
        settings
    }

    /// Replaces one value without snapping it. For [`NameplateSettings::validated`], which is
    /// putting a default back rather than accepting a player's number.
    #[must_use]
    fn set_exact(mut self, setting: NameplateSetting, value: f32) -> Self {
        match setting {
            NameplateSetting::Size => self.size = value,
            NameplateSetting::HeightAboveHead => self.height_above_head = value,
            NameplateSetting::NonTargetOpacity => self.non_target_opacity_in_combat = value,
            NameplateSetting::VerticalMargin => self.vertical_margin = value,
            NameplateSetting::HorizontalMargin => self.horizontal_margin = value,
            NameplateSetting::TopSafeZone => self.top_safe_zone = value,
        }
        self
    }

    /// Whether every field is finite and within its bounds.
    #[must_use]
    pub fn is_valid(self) -> bool {
        self == self.validated()
    }

    /// What the plate's text and box are scaled by, relative to the interface's own size.
    ///
    /// One at the default size, so a fresh install draws the plate exactly as it was drawn
    /// before the setting existed.
    #[must_use]
    pub fn text_scale(self) -> f32 {
        (self.size / NAMEPLATE_SIZE_DEFAULT).max(0.0)
    }

    /// How far above the head the plate is anchored, in metres.
    #[must_use]
    pub fn height_metres(self) -> f32 {
        self.height_above_head * NAMEPLATE_HEIGHT_UNIT
    }

    /// The alpha a non-target plate is drawn at while the player is in combat, in `[0, 1]`.
    #[must_use]
    pub fn non_target_alpha(self) -> f32 {
        (self.non_target_opacity_in_combat / 100.0).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_ones_the_reference_layout_prints() {
        let defaults = NameplateSettings::default();
        assert_eq!(defaults.size, 11.0);
        assert_eq!(defaults.height_above_head, 4.0);
        assert_eq!(defaults.non_target_opacity_in_combat, 40.0);
        assert_eq!(defaults.vertical_margin, 0.93);
        assert_eq!(defaults.horizontal_margin, 0.63);
        assert_eq!(defaults.top_safe_zone, 0.07);
        assert!(defaults.is_valid());
    }

    #[test]
    fn the_defaults_convert_to_exactly_what_the_game_already_did() {
        // The whole reason the numbers are what they are: a fresh install must not change
        // how a nameplate looks or where it floats.
        let defaults = NameplateSettings::default();
        assert!((defaults.text_scale() - 1.0).abs() < 1e-6);
        assert!((defaults.height_metres() - 0.9).abs() < 1e-6);
        assert!((defaults.non_target_alpha() - 0.4).abs() < 1e-6);
    }

    #[test]
    fn every_setting_has_a_default_inside_its_own_range_and_a_positive_step() {
        for setting in NameplateSetting::ALL {
            assert!(setting.minimum() < setting.maximum(), "{setting:?}");
            assert!(setting.step() > 0.0, "{setting:?}");
            let default = setting.default_value();
            assert!(default >= setting.minimum(), "{setting:?}");
            assert!(default <= setting.maximum(), "{setting:?}");
            assert_eq!(
                NameplateSettings::default().get(setting),
                default,
                "{setting:?} disagrees with the struct's default"
            );
        }
    }

    #[test]
    fn a_value_is_snapped_into_range_and_onto_its_step() {
        assert_eq!(NameplateSetting::Size.snap(-50.0), NAMEPLATE_SIZE_MIN);
        assert_eq!(NameplateSetting::Size.snap(9_000.0), NAMEPLATE_SIZE_MAX);
        assert_eq!(NameplateSetting::Size.snap(11.4), 11.0);
        assert!((NameplateSetting::TopSafeZone.snap(0.074) - 0.07).abs() < 1e-6);
        assert_eq!(
            NameplateSetting::VerticalMargin.snap(f32::NAN),
            NAMEPLATE_VERTICAL_MARGIN_DEFAULT
        );
    }

    #[test]
    fn every_snapped_value_is_one_the_loader_will_accept() {
        // Or a dragged slider could write a file the next launch throws away.
        for setting in NameplateSetting::ALL {
            let mut value = setting.minimum();
            while value <= setting.maximum() {
                let settings = NameplateSettings::default().with(setting, value);
                assert!(
                    settings.is_valid(),
                    "{setting:?} at {value} would not reload"
                );
                value += setting.step();
            }
        }
    }

    #[test]
    fn setting_one_value_leaves_the_other_five_alone() {
        let settings = NameplateSettings::default()
            .with(NameplateSetting::Size, 18.0)
            .with(NameplateSetting::TopSafeZone, 0.2);
        assert_eq!(settings.size, 18.0);
        assert!((settings.top_safe_zone - 0.2).abs() < 1e-6);
        assert_eq!(settings.height_above_head, NAMEPLATE_HEIGHT_DEFAULT);
        assert_eq!(settings.vertical_margin, NAMEPLATE_VERTICAL_MARGIN_DEFAULT);
    }

    #[test]
    fn one_nonsense_value_does_not_take_the_others_with_it() {
        let settings = NameplateSettings {
            size: f32::NAN,
            height_above_head: -5.0,
            vertical_margin: 1.5,
            ..NameplateSettings::default()
        }
        .validated();
        assert_eq!(settings.size, NAMEPLATE_SIZE_DEFAULT);
        assert_eq!(settings.height_above_head, NAMEPLATE_HEIGHT_DEFAULT);
        assert_eq!(settings.vertical_margin, 1.5, "a valid value was discarded");
    }

    #[test]
    fn the_settings_round_trip_through_json_under_their_own_names() {
        let settings = NameplateSettings::default().with(NameplateSetting::Size, 14.0);
        let text = serde_json::to_string(&settings).expect("nameplate settings serialize");
        assert!(text.contains("height_above_head"), "{text}");
        assert!(text.contains("non_target_opacity_in_combat"), "{text}");
        let parsed: NameplateSettings = serde_json::from_str(&text).expect("they parse");
        assert_eq!(parsed, settings);
    }

    #[test]
    fn a_file_missing_every_nameplate_field_loads_the_defaults() {
        let parsed: NameplateSettings = serde_json::from_str("{}").expect("an empty object");
        assert_eq!(parsed, NameplateSettings::default());
    }

    #[test]
    fn a_field_of_the_wrong_type_costs_that_field_and_nothing_else() {
        let parsed: NameplateSettings =
            serde_json::from_str("{\"size\": \"big\", \"vertical_margin\": 1.5}")
                .expect("one bad field does not fail the document");
        assert_eq!(
            parsed.validated().size,
            NAMEPLATE_SIZE_DEFAULT,
            "a string size was kept"
        );
        assert_eq!(
            parsed.validated().vertical_margin,
            1.5,
            "a valid neighbour was thrown away"
        );
    }
}
