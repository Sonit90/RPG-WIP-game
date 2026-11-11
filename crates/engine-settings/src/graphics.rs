//! Graphics settings: what the player trades quality for frame rate with, and how fast the
//! game is allowed to run.
//!
//! Shadows are deliberately a *word* rather than a pile of
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
//! ShadowPreset::High    the renderer: three cascades at 1536, contact shadows on
//! ```
//!
//! The frame-rate limit is the other half of the same trade, seen from the player's side: a
//! number they picked (`"max_fps": 144`, or `"unlimited"`), and a switch that drops the game
//! to [`BACKGROUND_FPS`] while its window is behind another one. Which limit is in force at a
//! given moment is [`GraphicsSettings::frame_rate_cap`]; holding the loop to it is the
//! platform layer's job.

use serde::{Deserialize, Serialize};

use crate::lenient_true;

/// The frame rate the game is held to while its window does not have focus, with energy
/// saving on.
///
/// Enough that the world behind another window still visibly moves; low enough that a game
/// left running behind a browser stops heating the machine it is on.
pub const BACKGROUND_FPS: u32 = 30;

/// The most frames a second the game draws.
///
/// A fixed list rather than any number: these are the refresh rates monitors are sold at,
/// plus 90 for a laptop on battery and 480 for a player who wants the cap only as a ceiling.
/// Written to the file as the number itself, or as `"unlimited"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameRateLimit {
    /// 60 frames a second.
    Fps60,
    /// 90 frames a second.
    Fps90,
    /// 144 frames a second.
    Fps144,
    /// 165 frames a second.
    Fps165,
    /// 180 frames a second.
    Fps180,
    /// 240 frames a second.
    Fps240,
    /// 480 frames a second.
    Fps480,
    /// No limit. The default: a fresh install runs exactly as fast as it did before the
    /// setting existed.
    #[default]
    Unlimited,
}

impl FrameRateLimit {
    /// Every limit, slowest first with unlimited last — the order the menu lists them in.
    pub const ALL: [Self; 8] = [
        Self::Fps60,
        Self::Fps90,
        Self::Fps144,
        Self::Fps165,
        Self::Fps180,
        Self::Fps240,
        Self::Fps480,
        Self::Unlimited,
    ];

    /// Frames a second, or `None` for no limit.
    #[must_use]
    pub fn fps(self) -> Option<u32> {
        match self {
            Self::Fps60 => Some(60),
            Self::Fps90 => Some(90),
            Self::Fps144 => Some(144),
            Self::Fps165 => Some(165),
            Self::Fps180 => Some(180),
            Self::Fps240 => Some(240),
            Self::Fps480 => Some(480),
            Self::Unlimited => None,
        }
    }

    /// The limit of exactly `fps` frames a second, or `None` if that is not one offered.
    #[must_use]
    pub fn from_fps(fps: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|limit| limit.fps() == Some(fps))
    }

    /// What the menu calls it. Upper case, like every label in the menu.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Fps60 => "60 FPS",
            Self::Fps90 => "90 FPS",
            Self::Fps144 => "144 FPS",
            Self::Fps165 => "165 FPS",
            Self::Fps180 => "180 FPS",
            Self::Fps240 => "240 FPS",
            Self::Fps480 => "480 FPS",
            Self::Unlimited => "UNLIMITED",
        }
    }
}

impl Serialize for FrameRateLimit {
    /// The number, or `"unlimited"`: what a person editing the file would write.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.fps() {
            Some(fps) => serializer.serialize_u32(fps),
            None => serializer.serialize_str("unlimited"),
        }
    }
}

impl<'de> Deserialize<'de> for FrameRateLimit {
    /// Leniently, like every other value in the file: a limit that is not one offered, or not
    /// a limit at all, costs this field — which becomes unlimited — and not the document.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let limit = match &value {
            serde_json::Value::Null => Some(Self::Unlimited),
            serde_json::Value::String(word) if word.trim().eq_ignore_ascii_case("unlimited") => {
                Some(Self::Unlimited)
            }
            serde_json::Value::Number(number) => number
                .as_u64()
                .and_then(|fps| u32::try_from(fps).ok())
                .and_then(Self::from_fps),
            _ => None,
        };
        Ok(limit.unwrap_or_else(|| {
            log::warn!(
                "settings: max_fps {value} is not one of the offered limits; using unlimited"
            );
            Self::Unlimited
        }))
    }
}

/// How much the renderer spends on shadows.
///
/// The five levels §17 of the shadow plan names. Serialized as a lower-case word, because
/// `settings.cfg` is meant to be opened and read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShadowQuality {
    /// No shadow rendering at all: no atlas, no depth passes, no sampling.
    Off,
    /// Two cascades at 1024 over 35 m, a single filtered comparison, no local-light shadows.
    Low,
    /// Three cascades at 1024, a four-tap disc, and four local shadow views for spot lights.
    Medium,
    /// Three cascades at 1536, a four-tap disc, eight local shadow views, contact shadows.
    ///
    /// The default. It is what the engine is tuned against, and it is affordable on the
    /// mid-range hardware this project targets.
    #[default]
    High,
    /// Four cascades at 2048 over 320 m, a six-tap disc, sixteen local shadow views.
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

    /// One line saying what this level buys, for the settings screen to print under it.
    ///
    /// Written in what a player can see rather than in what the renderer does: "soft
    /// shadows" and "shadows where things touch" are the difference they will notice, and
    /// "three cascades at 1536 with a four-tap disc" is not. The numbers behind each word
    /// live in `ShadowPreset::settings` in the renderer and nowhere else.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Off => "No shadows at all. The cheapest the game gets.",
            Self::Low => "Hard shadows, close to the camera only.",
            Self::Medium => "Soft shadows further out, and shadows from spot lights.",
            Self::High => "Sharper shadows, and shadows where things touch the ground.",
            Self::Ultra => "The longest range, the softest edges, and every kind of light.",
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

/// What the player can change about how the game looks, and how fast it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    /// How much is spent on shadows.
    pub shadows: ShadowQuality,
    /// The most frames a second the game draws while its window has focus.
    pub max_fps: FrameRateLimit,
    /// Whether the game drops to [`BACKGROUND_FPS`] while its window does not have focus.
    ///
    /// On by default: a game nobody is looking at has no reason to run a GPU flat out.
    #[serde(deserialize_with = "lenient_true")]
    pub preserve_energy_in_background: bool,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            shadows: ShadowQuality::default(),
            max_fps: FrameRateLimit::default(),
            preserve_energy_in_background: true,
        }
    }
}

impl GraphicsSettings {
    /// The frame rate the game should be held to right now, or `None` for no limit.
    ///
    /// The player's limit while the window has focus. In the background, with energy saving
    /// on, [`BACKGROUND_FPS`] — unless the player's own limit is lower still, because saving
    /// energy must never make the game run *faster* than it was asked to.
    #[must_use]
    pub fn frame_rate_cap(self, focused: bool) -> Option<u32> {
        let limit = self.max_fps.fps();
        if focused || !self.preserve_energy_in_background {
            return limit;
        }
        Some(limit.map_or(BACKGROUND_FPS, |fps| fps.min(BACKGROUND_FPS)))
    }

    /// These settings with anything unusable replaced by its default.
    ///
    /// Nothing here can currently be out of range — serde rejects a word that is not a
    /// level, `#[serde(default)]` turns that into the default, and a frame-rate limit that is
    /// not offered is read as unlimited — so this exists to keep
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
    fn every_level_says_what_it_buys() {
        // The settings screen prints these, so a level with nothing to say is a blank line
        // under a row the player is being asked to choose between.
        for quality in ShadowQuality::ALL {
            let description = quality.description();
            assert!(
                !description.is_empty(),
                "{} has no description",
                quality.as_str()
            );
            assert!(
                description.len() < 70,
                "{}'s description does not fit the panel: {description:?}",
                quality.as_str()
            );
        }
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
            ..GraphicsSettings::default()
        })
        .expect("graphics settings serialize");
        assert!(json.contains("\"medium\""), "{json}");
    }

    #[test]
    fn a_fresh_install_is_unlimited_and_saves_energy_in_the_background() {
        let defaults = GraphicsSettings::default();
        assert_eq!(defaults.max_fps, FrameRateLimit::Unlimited);
        assert!(defaults.preserve_energy_in_background);
    }

    #[test]
    fn the_limits_are_offered_slowest_first_with_unlimited_last() {
        let numbers: Vec<u32> = FrameRateLimit::ALL
            .iter()
            .filter_map(|limit| limit.fps())
            .collect();
        assert_eq!(numbers, [60, 90, 144, 165, 180, 240, 480]);
        assert_eq!(FrameRateLimit::ALL.last(), Some(&FrameRateLimit::Unlimited));
        for limit in FrameRateLimit::ALL {
            if let Some(fps) = limit.fps() {
                assert_eq!(FrameRateLimit::from_fps(fps), Some(limit));
            }
            assert!(!limit.label().is_empty());
        }
        assert_eq!(FrameRateLimit::from_fps(123), None);
    }

    #[test]
    fn the_frame_rate_settings_round_trip_through_the_file() {
        for limit in FrameRateLimit::ALL {
            for preserve in [true, false] {
                let settings = GraphicsSettings {
                    max_fps: limit,
                    preserve_energy_in_background: preserve,
                    ..GraphicsSettings::default()
                };
                let json = serde_json::to_string(&settings).expect("graphics settings serialize");
                let parsed: GraphicsSettings =
                    serde_json::from_str(&json).expect("graphics settings parse");
                assert_eq!(parsed, settings, "{json}");
            }
        }
    }

    #[test]
    fn the_file_writes_a_limit_as_its_number_and_no_limit_as_a_word() {
        let at = |max_fps| {
            serde_json::to_string(&GraphicsSettings {
                max_fps,
                ..GraphicsSettings::default()
            })
            .expect("graphics settings serialize")
        };
        assert!(at(FrameRateLimit::Fps144).contains("\"max_fps\":144"));
        assert!(at(FrameRateLimit::Unlimited).contains("\"max_fps\":\"unlimited\""));
        assert!(at(FrameRateLimit::Unlimited).contains("\"preserve_energy_in_background\":true"));
    }

    #[test]
    fn a_limit_that_is_not_offered_costs_only_that_field() {
        // A hand-edit: 123 is not a choice, so it reads as unlimited, and the shadows beside
        // it still load.
        let parsed: GraphicsSettings = serde_json::from_str(
            "{\"shadows\": \"low\", \"max_fps\": 123, \"preserve_energy_in_background\": false}",
        )
        .expect("one wrong field does not fail the section");
        assert_eq!(parsed.shadows, ShadowQuality::Low);
        assert_eq!(parsed.max_fps, FrameRateLimit::Unlimited);
        assert!(!parsed.preserve_energy_in_background);
        for odd in ["\"fast\"", "-60", "60.5", "true", "[60]"] {
            let parsed: GraphicsSettings =
                serde_json::from_str(&format!("{{\"max_fps\": {odd}}}")).expect("lenient");
            assert_eq!(parsed.max_fps, FrameRateLimit::Unlimited, "{odd}");
        }
        let words: GraphicsSettings = serde_json::from_str(
            "{\"max_fps\": \" Unlimited \", \"preserve_energy_in_background\": \"yes\"}",
        )
        .expect("lenient");
        assert_eq!(words.max_fps, FrameRateLimit::Unlimited);
        assert!(
            words.preserve_energy_in_background,
            "an unreadable switch is on"
        );
    }

    #[test]
    fn a_focused_window_runs_at_the_players_limit() {
        for limit in FrameRateLimit::ALL {
            for preserve in [true, false] {
                let settings = GraphicsSettings {
                    max_fps: limit,
                    preserve_energy_in_background: preserve,
                    ..GraphicsSettings::default()
                };
                assert_eq!(settings.frame_rate_cap(true), limit.fps());
            }
        }
    }

    #[test]
    fn the_background_is_held_to_thirty_and_never_raised() {
        let background = |max_fps| {
            GraphicsSettings {
                max_fps,
                ..GraphicsSettings::default()
            }
            .frame_rate_cap(false)
        };
        assert_eq!(background(FrameRateLimit::Unlimited), Some(BACKGROUND_FPS));
        assert_eq!(background(FrameRateLimit::Fps144), Some(BACKGROUND_FPS));
        assert_eq!(background(FrameRateLimit::Fps60), Some(BACKGROUND_FPS));
        // Every offered limit is above thirty, so whatever the player chose, the background
        // is never faster than it.
        for limit in FrameRateLimit::ALL {
            let cap = background(limit).expect("the background is always limited");
            assert!(limit.fps().is_none_or(|fps| cap <= fps), "{limit:?}");
        }
    }

    #[test]
    fn with_energy_saving_off_the_background_runs_like_the_foreground() {
        for limit in FrameRateLimit::ALL {
            let settings = GraphicsSettings {
                max_fps: limit,
                preserve_energy_in_background: false,
                ..GraphicsSettings::default()
            };
            assert_eq!(settings.frame_rate_cap(false), limit.fps());
        }
    }

    #[test]
    fn a_file_with_no_graphics_section_loads_the_defaults() {
        let settings: GraphicsSettings =
            serde_json::from_str("{}").expect("an empty object is a valid section");
        assert_eq!(settings, GraphicsSettings::default());
    }
}
