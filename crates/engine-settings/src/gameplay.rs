//! Gameplay settings: the rules of the game a player is allowed to change.
//!
//! Small on purpose. This is not a difficulty system and not a cheat menu — it is the handful
//! of numbers whose right value depends on how somebody plays rather than on how the game is
//! balanced, starting with how fast the character mends between fights.
//!
//! The defaults live in [`engine_core::tuning::player`], with every other default in the
//! project; this module is where they are read from `settings.cfg` and validated.

use engine_core::tuning::player as tuning;
use serde::{Deserialize, Serialize};

use crate::lenient;

/// Health regained per second out of combat on a fresh install, as a percentage.
pub const HEALTH_REGEN_PERCENT_DEFAULT: f32 = tuning::HEALTH_REGEN_PER_SECOND * 100.0;

/// Least regeneration the file accepts: none at all.
pub const HEALTH_REGEN_PERCENT_MIN: f32 = tuning::HEALTH_REGEN_PERCENT_MIN;

/// Most regeneration the file accepts, as a percentage a second.
pub const HEALTH_REGEN_PERCENT_MAX: f32 = tuning::HEALTH_REGEN_PERCENT_MAX;

/// Granularity of the regeneration setting.
pub const HEALTH_REGEN_PERCENT_STEP: f32 = tuning::HEALTH_REGEN_PERCENT_STEP;

/// What the player can change about how the game plays.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameplaySettings {
    /// Health the player regains per second out of combat, as a percentage of their maximum.
    ///
    /// A percentage because that is the number a player says out loud — "one percent a
    /// second" — and because it is independent of whatever the character's maximum health
    /// turns out to be. The simulation works in fractions; see
    /// [`GameplaySettings::health_regen_fraction`].
    #[serde(deserialize_with = "lenient")]
    pub health_regen_percent_per_second: f32,
}

impl Default for GameplaySettings {
    fn default() -> Self {
        Self {
            health_regen_percent_per_second: HEALTH_REGEN_PERCENT_DEFAULT,
        }
    }
}

impl GameplaySettings {
    /// These settings with every out-of-range or non-finite field replaced by its default.
    #[must_use]
    pub fn validated(self) -> Self {
        let mut settings = self;
        let regen = settings.health_regen_percent_per_second;
        if !regen.is_finite()
            || !(HEALTH_REGEN_PERCENT_MIN..=HEALTH_REGEN_PERCENT_MAX).contains(&regen)
        {
            log::warn!(
                "settings: health_regen_percent_per_second {regen} is outside \
                 {HEALTH_REGEN_PERCENT_MIN}..{HEALTH_REGEN_PERCENT_MAX}; using the default \
                 {HEALTH_REGEN_PERCENT_DEFAULT}"
            );
            settings.health_regen_percent_per_second = HEALTH_REGEN_PERCENT_DEFAULT;
        }
        settings
    }

    /// Whether every field is finite and within its bounds.
    #[must_use]
    pub fn is_valid(self) -> bool {
        self == self.validated()
    }

    /// The regeneration rate as the fraction of maximum health per second the simulation
    /// works in.
    #[must_use]
    pub fn health_regen_fraction(self) -> f32 {
        (self.health_regen_percent_per_second / 100.0).max(0.0)
    }

    /// `value` clamped into range and snapped onto the setting's step.
    #[must_use]
    pub fn snap_health_regen_percent(value: f32) -> f32 {
        if !value.is_finite() {
            return HEALTH_REGEN_PERCENT_DEFAULT;
        }
        let steps = ((value - HEALTH_REGEN_PERCENT_MIN) / HEALTH_REGEN_PERCENT_STEP).round();
        (HEALTH_REGEN_PERCENT_MIN + steps * HEALTH_REGEN_PERCENT_STEP)
            .clamp(HEALTH_REGEN_PERCENT_MIN, HEALTH_REGEN_PERCENT_MAX)
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
    fn the_default_is_one_percent_a_second_and_converts_to_a_hundredth() {
        let defaults = GameplaySettings::default();
        assert!((defaults.health_regen_percent_per_second - 1.0).abs() < 1e-6);
        assert!((defaults.health_regen_fraction() - 0.01).abs() < 1e-9);
        assert!(defaults.is_valid());
    }

    #[test]
    fn an_out_of_range_or_meaningless_rate_falls_back_to_the_default() {
        for bad in [-5.0, 1_000.0, f32::NAN, f32::INFINITY] {
            let settings = GameplaySettings {
                health_regen_percent_per_second: bad,
            };
            assert_eq!(
                settings.validated().health_regen_percent_per_second,
                HEALTH_REGEN_PERCENT_DEFAULT,
                "{bad}"
            );
        }
    }

    #[test]
    fn a_field_of_the_wrong_type_loads_as_the_default_rather_than_failing_the_file() {
        let parsed: GameplaySettings =
            serde_json::from_str("{\"health_regen_percent_per_second\": \"fast\"}")
                .expect("one bad field does not fail the document");
        assert_eq!(
            parsed.validated().health_regen_percent_per_second,
            HEALTH_REGEN_PERCENT_DEFAULT
        );
    }

    #[test]
    fn a_rate_is_snapped_into_range_and_onto_its_step() {
        assert_eq!(
            GameplaySettings::snap_health_regen_percent(-1.0),
            HEALTH_REGEN_PERCENT_MIN
        );
        assert_eq!(
            GameplaySettings::snap_health_regen_percent(999.0),
            HEALTH_REGEN_PERCENT_MAX
        );
        assert!((GameplaySettings::snap_health_regen_percent(2.53) - 2.5).abs() < 1e-5);
    }

    #[test]
    fn turning_regeneration_off_is_a_legal_setting() {
        let settings = GameplaySettings {
            health_regen_percent_per_second: 0.0,
        };
        assert!(settings.is_valid());
        assert_eq!(settings.health_regen_fraction(), 0.0);
    }
}
