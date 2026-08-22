//! Graphics settings: what the player trades quality for frame rate with.
//!
//! One setting so far — shadows — and it is deliberately a *word* rather than a pile of
//! numbers. Cascade counts, shadow-map resolutions, filter widths and biases are engine
//! configuration; "high" is what a player chooses. The renderer maps the word onto the
//! numbers in exactly one place, `ShadowPreset::settings`, so a quality level cannot promise
//! something the renderer does not implement and the file cannot ask for a configuration
//! that has never been run.
//!
//! ```text
//! settings.cfg          "shadows": "high"
//!        |
//! ShadowQuality::High   this module: a word the player picked
//!        |              (the application converts, because it is the one place that knows
//!        v               both this crate and the renderer)
//! ShadowPreset::High    the renderer: three cascades at 2048, contact shadows on
//! ```

use serde::{Deserialize, Serialize};

/// How much the renderer spends on shadows.
///
/// The five levels §17 of the shadow plan names. Serialized as a lower-case word, because
/// `settings.cfg` is meant to be opened and read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShadowQuality {
    /// No shadow rendering at all: no atlas, no depth passes, no sampling.
    Off,
    /// Two cascades at 1024, a single filtered comparison, no local-light shadows.
    Low,
    /// Three cascades at 1024, and two shadow maps for local lights.
    Medium,
    /// Three cascades at 2048, four local shadow maps, and contact shadows.
    ///
    /// The default. It is what the engine is tuned against, and it is affordable on the
    /// mid-range hardware this project targets.
    #[default]
    High,
    /// Four cascades at 2048, a wider filter, eight local shadow maps.
    Ultra,
}

impl ShadowQuality {
    /// Every level, cheapest first — the order a settings menu lists them in.
    pub const ALL: [Self; 5] = [Self::Off, Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// The word this level is written as, in the file and in a log line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }

    /// The level a word names, or `None`.
    ///
    /// Case-insensitive and trimmed, because the file is hand-edited.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|quality| quality.as_str().eq_ignore_ascii_case(name.trim()))
    }

    /// The next level up, wrapping back to `Off` past the top.
    ///
    /// What a developer key cycles through. Wrapping rather than saturating so one key can
    /// reach every level, including "off", which is the one worth reaching quickly when a
    /// frame is slow.
    #[must_use]
    pub fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|level| *level == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// What the player can change about how the game looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    /// How much is spent on shadows.
    pub shadows: ShadowQuality,
}

impl GraphicsSettings {
    /// These settings with anything unusable replaced by its default.
    ///
    /// Nothing here can currently be out of range — serde rejects a word that is not a
    /// level, and `#[serde(default)]` turns that into the default — so this exists to keep
    /// the shape of every other settings section, and to be the place a future numeric
    /// setting is validated in.
    #[must_use]
    pub fn validated(self) -> Self {
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

    #[test]
    fn the_default_is_high() {
        // The level the engine is tuned against. A fresh install should look the way the
        // screenshots do.
        assert_eq!(GraphicsSettings::default().shadows, ShadowQuality::High);
    }

    #[test]
    fn levels_round_trip_through_their_names() {
        for quality in ShadowQuality::ALL {
            assert_eq!(ShadowQuality::from_name(quality.as_str()), Some(quality));
        }
        assert_eq!(
            ShadowQuality::from_name(" ULTRA "),
            Some(ShadowQuality::Ultra)
        );
        assert_eq!(ShadowQuality::from_name("cinematic"), None);
    }

    #[test]
    fn cycling_reaches_every_level_and_returns() {
        let mut level = ShadowQuality::Off;
        let mut seen = vec![level];
        for _ in 0..ShadowQuality::ALL.len() - 1 {
            level = level.next();
            seen.push(level);
        }
        assert_eq!(seen, ShadowQuality::ALL.to_vec());
        assert_eq!(level.next(), ShadowQuality::Off, "the cycle wraps");
    }

    #[test]
    fn the_file_spells_the_level_as_a_word() {
        // The file is meant to be readable. A numeric enum discriminant would make an
        // ordinary edit into a lookup.
        let json = serde_json::to_string(&GraphicsSettings {
            shadows: ShadowQuality::Medium,
        })
        .expect("graphics settings serialize");
        assert!(json.contains("\"medium\""), "{json}");
    }

    #[test]
    fn a_file_with_no_graphics_section_loads_the_defaults() {
        let settings: GraphicsSettings =
            serde_json::from_str("{}").expect("an empty object is a valid section");
        assert_eq!(settings, GraphicsSettings::default());
    }
}
