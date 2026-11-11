//! Floating combat text settings: which numbers float up, and how big they are.
//!
//! ```text
//! settings.cfg   "combat_text": { "enabled": true, "outgoing_damage": true, ..., "size": 100 }
//!        |
//! CombatTextSettings    this module: five switches and a size, in the units the file prints
//!        |
//! engine-presentation's combat text reads them every frame; nothing is cached against them.
//! ```
//!
//! Every field loads leniently, as the nameplate section's do: a switch that is not a switch or
//! a size that is not a number costs that field alone and never the rest of the file.

use engine_core::tuning::combat_text as tuning;
use serde::{Deserialize, Serialize};

use crate::{lenient, lenient_true};

/// The text size a fresh install draws at, as a percentage of the design size.
pub const COMBAT_TEXT_SIZE_DEFAULT: f32 = tuning::SIZE_PERCENT_DEFAULT;

/// The smallest the text can be set, as a percentage.
pub const COMBAT_TEXT_SIZE_MIN: f32 = tuning::SIZE_PERCENT_MIN;

/// The largest the text can be set, as a percentage.
pub const COMBAT_TEXT_SIZE_MAX: f32 = tuning::SIZE_PERCENT_MAX;

/// Granularity of the size, in percent.
pub const COMBAT_TEXT_SIZE_STEP: f32 = tuning::SIZE_PERCENT_STEP;

/// What the player can change about floating combat text.
///
/// The defaults show everything at the design size, because a fresh install should read
/// immediately: a player who has to find a setting before their damage is visible has been
/// told the game is unfinished.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatTextSettings {
    /// Whether any combat text is drawn. Off silences every category below.
    #[serde(deserialize_with = "lenient_true")]
    pub enabled: bool,
    /// Damage the player deals.
    #[serde(deserialize_with = "lenient_true")]
    pub outgoing_damage: bool,
    /// Damage the player takes, and the attacks they avoid.
    #[serde(deserialize_with = "lenient_true")]
    pub incoming_damage: bool,
    /// Healing the player does, and healing they receive.
    #[serde(deserialize_with = "lenient_true")]
    pub healing: bool,
    /// Whether a critical strike is drawn larger and pops when it appears.
    ///
    /// Off, a critical keeps its colour and loses the size and the pop — for a player who
    /// finds the motion distracting.
    #[serde(deserialize_with = "lenient_true")]
    pub critical_emphasis: bool,
    /// How big the text is, as a percentage of its design size, on top of the interface scale.
    #[serde(deserialize_with = "lenient")]
    pub size: f32,
}

impl Default for CombatTextSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            outgoing_damage: true,
            incoming_damage: true,
            healing: true,
            critical_emphasis: true,
            size: COMBAT_TEXT_SIZE_DEFAULT,
        }
    }
}

impl CombatTextSettings {
    /// These settings with an out-of-range or non-finite size replaced by its default.
    #[must_use]
    pub fn validated(self) -> Self {
        let mut settings = self;
        if !settings.size.is_finite()
            || !(COMBAT_TEXT_SIZE_MIN..=COMBAT_TEXT_SIZE_MAX).contains(&settings.size)
        {
            log::warn!(
                "settings: combat_text.size {} is outside {COMBAT_TEXT_SIZE_MIN}..\
                 {COMBAT_TEXT_SIZE_MAX}; using the default {COMBAT_TEXT_SIZE_DEFAULT}",
                settings.size
            );
            settings.size = COMBAT_TEXT_SIZE_DEFAULT;
        }
        settings
    }

    /// Whether every field is usable.
    #[must_use]
    pub fn is_valid(self) -> bool {
        self == self.validated()
    }

    /// What the text's design sizes are multiplied by, before the interface scale.
    #[must_use]
    pub fn size_scale(self) -> f32 {
        (self.size / 100.0).max(0.0)
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
    fn a_fresh_install_shows_everything_at_the_design_size() {
        let defaults = CombatTextSettings::default();
        assert!(defaults.enabled);
        assert!(defaults.outgoing_damage && defaults.incoming_damage && defaults.healing);
        assert!(defaults.critical_emphasis);
        assert!((defaults.size_scale() - 1.0).abs() < 1e-6);
        assert!(defaults.is_valid());
    }

    #[test]
    fn a_missing_section_and_an_empty_one_are_both_the_defaults() {
        let parsed: CombatTextSettings = serde_json::from_str("{}").expect("an empty object");
        assert_eq!(parsed, CombatTextSettings::default());
    }

    #[test]
    fn one_bad_field_costs_only_itself() {
        let parsed: CombatTextSettings = serde_json::from_str(
            "{\"enabled\": \"on\", \"healing\": false, \"size\": 900, \"critical_emphasis\": 0}",
        )
        .expect("one bad field does not fail the section");
        let loaded = parsed.validated();
        assert!(loaded.enabled, "a word is not a switch; the default stands");
        assert!(!loaded.healing, "a real switch was thrown away");
        assert!(loaded.critical_emphasis);
        assert_eq!(loaded.size, COMBAT_TEXT_SIZE_DEFAULT);
    }

    #[test]
    fn the_settings_round_trip_under_their_own_names() {
        let settings = CombatTextSettings {
            incoming_damage: false,
            size: 140.0,
            ..CombatTextSettings::default()
        };
        let text = serde_json::to_string(&settings).expect("encodes");
        assert!(text.contains("\"incoming_damage\":false"), "{text}");
        let back: CombatTextSettings = serde_json::from_str(&text).expect("decodes");
        assert_eq!(back, settings);
    }
}
