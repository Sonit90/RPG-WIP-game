//! The mixer: one master volume and one per category.
//!
//! ```text
//!  voice ─ its own gain ─┐
//!  voice ─ its own gain ─┼─ category volume ─┬─ master volume ─▶ the device
//!  voice ─ its own gain ─┘   (SFX/UI/Ambient)│
//! ```
//!
//! Deliberately not a DAW. There is no routing graph, no effects chain, no send, no bus
//! hierarchy deeper than one level. What the requirement actually is — *independent volume
//! control that survives a restart* — is four numbers, and four numbers is what this is.
//!
//! # Every value is validated on the way in
//!
//! A volume is a finite number in `[0, 1]`. A NaN, an infinity or a value out of range is
//! replaced by the default rather than clamped, for the same reason
//! [`engine_settings::GameSettings::validated`](../../engine_settings) replaces rather than
//! clamps: `-100` is not a request for silence, it is a value that means nothing, and the
//! honest answer to a meaningless setting is the default.

use crate::asset::AudioCategory;

/// The volume a fresh install runs every bus at.
pub const DEFAULT_VOLUME: f32 = 1.0;

/// The quietest a volume can be: silence.
pub const VOLUME_MIN: f32 = 0.0;

/// The loudest. Above one is amplification, and amplification is what a limiter is for.
pub const VOLUME_MAX: f32 = 1.0;

/// Master and per-category volumes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mixer {
    master: f32,
    categories: [f32; 3],
    muted: bool,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            master: DEFAULT_VOLUME,
            categories: [DEFAULT_VOLUME; 3],
            muted: false,
        }
    }
}

impl Mixer {
    /// A mixer with every bus at its default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The master volume.
    #[must_use]
    pub fn master(&self) -> f32 {
        self.master
    }

    /// Sets the master volume. An invalid value becomes [`DEFAULT_VOLUME`].
    pub fn set_master(&mut self, volume: f32) {
        self.master = validated(volume);
    }

    /// One category's volume.
    #[must_use]
    pub fn category(&self, category: AudioCategory) -> f32 {
        self.categories[category.index()]
    }

    /// Sets one category's volume. An invalid value becomes [`DEFAULT_VOLUME`].
    pub fn set_category(&mut self, category: AudioCategory, volume: f32) {
        self.categories[category.index()] = validated(volume);
    }

    /// Whether everything is silenced without any volume having been changed.
    ///
    /// What alt-tabbing away uses: the player's volumes are still theirs when the window
    /// comes back.
    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Silences or unsilences everything.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// What a voice on `category` is multiplied by, master included.
    #[must_use]
    pub fn gain(&self, category: AudioCategory) -> f32 {
        if self.muted {
            return 0.0;
        }
        self.master * self.category(category)
    }

    /// Whether nothing on `category` could be heard.
    ///
    /// Used to skip work rather than to change behaviour: a sound asked for at zero volume
    /// is still started, still counted and still ends when it would have ended, so turning
    /// the volume back up mid-fight does not resurrect a backlog.
    #[must_use]
    pub fn is_silent(&self, category: AudioCategory) -> bool {
        self.gain(category) <= f32::EPSILON
    }
}

/// `volume` if it is a finite number in range, [`DEFAULT_VOLUME`] otherwise.
fn validated(volume: f32) -> f32 {
    if volume.is_finite() && (VOLUME_MIN..=VOLUME_MAX).contains(&volume) {
        volume
    } else {
        log::warn!(
            "audio: volume {volume} is outside {VOLUME_MIN}..{VOLUME_MAX}; using the default"
        );
        DEFAULT_VOLUME
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
    fn a_fresh_mixer_passes_everything_through_unchanged() {
        let mixer = Mixer::new();
        for category in AudioCategory::ALL {
            assert_eq!(mixer.gain(category), 1.0);
            assert!(!mixer.is_silent(category));
        }
    }

    #[test]
    fn master_and_category_multiply() {
        let mut mixer = Mixer::new();
        mixer.set_master(0.5);
        mixer.set_category(AudioCategory::Sfx, 0.5);
        assert!((mixer.gain(AudioCategory::Sfx) - 0.25).abs() < 1e-6);
        assert!((mixer.gain(AudioCategory::Ui) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn categories_are_independent() {
        let mut mixer = Mixer::new();
        mixer.set_category(AudioCategory::Ambient, 0.0);
        assert!(mixer.is_silent(AudioCategory::Ambient));
        assert!(!mixer.is_silent(AudioCategory::Sfx));
        assert!(!mixer.is_silent(AudioCategory::Ui));
    }

    #[test]
    fn an_invalid_volume_becomes_the_default_rather_than_a_nan() {
        let mut mixer = Mixer::new();
        for bad in [f32::NAN, f32::INFINITY, -1.0, 4.0] {
            mixer.set_master(bad);
            assert_eq!(mixer.master(), DEFAULT_VOLUME, "{bad}");
            mixer.set_category(AudioCategory::Sfx, bad);
            assert_eq!(mixer.category(AudioCategory::Sfx), DEFAULT_VOLUME);
        }
        // Zero is a legal request for silence and must survive.
        mixer.set_master(0.0);
        assert_eq!(mixer.master(), 0.0);
    }

    #[test]
    fn muting_silences_without_losing_the_players_volumes() {
        let mut mixer = Mixer::new();
        mixer.set_master(0.7);
        mixer.set_muted(true);
        assert_eq!(mixer.gain(AudioCategory::Sfx), 0.0);
        mixer.set_muted(false);
        assert!((mixer.gain(AudioCategory::Sfx) - 0.7).abs() < 1e-6);
    }
}
