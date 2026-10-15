//! Sounds, authored in code.
//!
//! There is not a single binary art asset in this repository. Meshes are written by
//! [`engine_assets::procedural`](../../engine_assets), icons are SVG rasterized by a build
//! script, and effects are a table of numbers. Sound follows the rule that was already here:
//! a [`Recipe`] is a handful of parameters, and [`render`] turns it into PCM at load.
//!
//! # Why one recipe and not one synthesiser per sound
//!
//! Every sound this class needs is the same three ingredients in different proportions:
//!
//! ```text
//!   filtered noise   ── the body of fire, wind, dust, an explosion's roar
//! + a swept sine     ── the thump under an impact, the whistle of something falling
//! + sparse impulses  ── crackle, embers, footsteps on grit
//!   × an envelope    ── what tells a burst from a bed
//! ```
//!
//! So there is one synthesiser and a table of parameters, which is the same shape as
//! `engine-vfx`'s effect catalogue. Making Meteor's explosion bigger is a number, not a new
//! function, and two explosions that should be related *are* related because they are the
//! same recipe at different sizes.
//!
//! # It is deterministic
//!
//! The noise comes from [`crate::Rng`] seeded per variation, so the bank a machine
//! synthesises is the bank every other machine synthesises. A test can assert on the peak
//! and the length of a sound and mean it.

use crate::rng::Rng;

/// The longest sound the synthesiser will produce, in seconds.
///
/// A guard rail, not a design limit: a recipe with a nonsense duration would otherwise
/// allocate for as long as the number said.
pub const MAX_DURATION: f32 = 8.0;

/// How much of a looping sound is crossfaded to hide the seam, in seconds.
///
/// Long enough that a noise bed has no click, short enough that a two-second loop still
/// spends most of its time being itself.
const LOOP_CROSSFADE: f32 = 0.12;

/// The peak every synthesised sound is normalised to.
///
/// Below one so that summing a few voices at their catalogue gains has headroom before the
/// mix limiter has anything to do.
const NORMALISED_PEAK: f32 = 0.85;

/// What a sound is made of.
///
/// Every field has a neutral value in [`Recipe::new`], so a recipe names only what makes it
/// itself. See the module docs for the three ingredients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recipe {
    /// How long it lasts, in seconds, before the loop crossfade.
    pub duration: f32,
    /// How long the level takes to reach full, in seconds.
    pub attack: f32,
    /// The fraction of the duration held at full before the decay starts.
    pub sustain: f32,
    /// The shape of the decay. 1 is linear; above 1 drops fast and leaves a tail.
    pub decay_shape: f32,
    /// Level of the noise bed.
    pub noise: f32,
    /// Lowpass cutoff applied to the noise at the start, in Hz.
    pub cutoff_start: f32,
    /// Lowpass cutoff at the end, in Hz. Sweeping down is what makes fire recede.
    pub cutoff_end: f32,
    /// Level of the swept sine.
    pub tone: f32,
    /// Where the sine starts, in Hz.
    pub tone_start_hz: f32,
    /// Where the sine ends, in Hz.
    pub tone_end_hz: f32,
    /// Level of a second sine riding the first at a fixed frequency ratio: what makes a
    /// chime a chime (ratio 2) or a struck piece of metal metal (ratio 2.76).
    pub harmonic: f32,
    /// That second sine's frequency as a multiple of the first's.
    pub harmonic_ratio: f32,
    /// How many separate notes the tone sweep is quantised into. Zero or one leaves it a
    /// smooth sweep; more turns it into an arpeggio of that many plucked, semitone-snapped
    /// notes from the start frequency to the end one.
    pub steps: u8,
    /// Level of the sparse impulses.
    pub crackle: f32,
    /// How many impulses a second.
    pub crackle_rate: f32,
    /// Rate of a slow level wobble, in Hz. Zero for none.
    pub tremolo_hz: f32,
    /// How deep that wobble is, 0 to 1.
    pub tremolo_depth: f32,
    /// Whether the end is crossfaded into the start so the sound can repeat seamlessly.
    pub looping: bool,
}

impl Recipe {
    /// A silent recipe of `duration` seconds. Every ingredient is off.
    #[must_use]
    pub const fn new(duration: f32) -> Self {
        Self {
            duration,
            attack: 0.004,
            sustain: 0.0,
            decay_shape: 2.0,
            noise: 0.0,
            cutoff_start: 8_000.0,
            cutoff_end: 8_000.0,
            tone: 0.0,
            tone_start_hz: 220.0,
            tone_end_hz: 220.0,
            harmonic: 0.0,
            harmonic_ratio: 2.0,
            steps: 0,
            crackle: 0.0,
            crackle_rate: 0.0,
            tremolo_hz: 0.0,
            tremolo_depth: 0.0,
            looping: false,
        }
    }

    /// With a noise bed filtered from `start` Hz down (or up) to `end` Hz.
    #[must_use]
    pub const fn noise(mut self, level: f32, start: f32, end: f32) -> Self {
        self.noise = level;
        self.cutoff_start = start;
        self.cutoff_end = end;
        self
    }

    /// With a sine sweeping from `start` Hz to `end` Hz.
    #[must_use]
    pub const fn tone(mut self, level: f32, start: f32, end: f32) -> Self {
        self.tone = level;
        self.tone_start_hz = start;
        self.tone_end_hz = end;
        self
    }

    /// With a second sine at `level`, `ratio` times the tone's frequency.
    #[must_use]
    pub const fn harmonic(mut self, level: f32, ratio: f32) -> Self {
        self.harmonic = level;
        self.harmonic_ratio = ratio;
        self
    }

    /// With the tone played as `count` plucked notes instead of one sweep.
    #[must_use]
    pub const fn steps(mut self, count: u8) -> Self {
        self.steps = count;
        self
    }

    /// With `rate` impulses a second at `level`.
    #[must_use]
    pub const fn crackle(mut self, level: f32, rate: f32) -> Self {
        self.crackle = level;
        self.crackle_rate = rate;
        self
    }

    /// With an envelope that rises over `attack` seconds and holds for `sustain` of the
    /// duration before decaying with `shape`.
    #[must_use]
    pub const fn envelope(mut self, attack: f32, sustain: f32, shape: f32) -> Self {
        self.attack = attack;
        self.sustain = sustain;
        self.decay_shape = shape;
        self
    }

    /// With a slow level wobble, which is what keeps a long bed from sounding synthetic.
    #[must_use]
    pub const fn tremolo(mut self, hz: f32, depth: f32) -> Self {
        self.tremolo_hz = hz;
        self.tremolo_depth = depth;
        self
    }

    /// Seamless: flat-topped and crossfaded, so it can repeat forever.
    #[must_use]
    pub const fn looped(mut self) -> Self {
        self.looping = true;
        self.attack = 0.0;
        self.sustain = 1.0;
        self
    }
}

/// Renders `recipe` to mono PCM at `sample_rate`, seeded by `seed`.
///
/// The result is normalised to a fixed peak and is never empty: a recipe asking for zero or
/// nonsense length still produces one frame of silence, because a zero-length asset would
/// have to be special-cased by every voice that played it.
#[must_use]
pub fn render(recipe: &Recipe, sample_rate: u32, seed: u64) -> Vec<f32> {
    let rate = sample_rate.max(1) as f32;
    let duration = if recipe.duration.is_finite() {
        recipe.duration.clamp(0.0, MAX_DURATION)
    } else {
        0.0
    };
    let crossfade = if recipe.looping {
        LOOP_CROSSFADE.min(duration * 0.5)
    } else {
        0.0
    };
    let body = (duration * rate).round() as usize;
    let fade = (crossfade * rate).round() as usize;
    if body == 0 {
        return vec![0.0];
    }

    let mut rng = Rng::new(seed);
    // Rendered one crossfade longer than it will be kept: the extra tail is what gets folded
    // back over the head so a loop has no seam.
    let total = body + fade;
    let mut samples = Vec::with_capacity(total);

    // One-pole lowpass state, and the phase of the swept sine. Both carry across the whole
    // render, which is why the loop below is a fold rather than a map.
    let mut filtered = 0.0_f32;
    let mut phase = 0.0_f32;
    let mut harmonic_phase = 0.0_f32;
    // Fractional impulses per sample; a counter crossing 1 fires one and keeps the remainder,
    // which is what makes a rate of 7.5 a second mean 7.5 a second rather than 7.
    let crackle_step = (recipe.crackle_rate.max(0.0)) / rate;
    let mut crackle_accum = 0.0_f32;
    let mut crackle_level = 0.0_f32;

    for index in 0..total {
        let time = index as f32 / rate;
        let fraction = (time / duration).clamp(0.0, 1.0);

        // Noise, lowpassed with a cutoff that moves across the sound. Recomputing the
        // coefficient per sample is a divide and an exp-free approximation; at 48 kHz over
        // at most a few seconds this is nothing, and it is what makes a sweep a sweep.
        let cutoff =
            lerp(recipe.cutoff_start, recipe.cutoff_end, fraction).clamp(20.0, rate * 0.45);
        let alpha = (1.0 - (-std::f32::consts::TAU * cutoff / rate).exp()).clamp(0.0, 1.0);
        filtered += alpha * (rng.signed() - filtered);
        // The one-pole loses most of its amplitude at low cutoffs; compensating keeps a
        // dark explosion as loud as a bright one instead of leaving it a whisper.
        let noise = filtered * (1.0 / alpha.max(0.05)).sqrt().min(6.0) * recipe.noise;

        // The swept sine. Frequency is interpolated exponentially so a drop from 300 Hz to
        // 40 Hz sounds like one steady fall rather than a cliff at the start.
        let stepped = recipe.steps > 1;
        let hz = if stepped {
            stepped_hz(recipe, fraction)
        } else {
            exp_lerp(recipe.tone_start_hz, recipe.tone_end_hz, fraction)
        };
        phase += std::f32::consts::TAU * hz / rate;
        if phase > std::f32::consts::TAU {
            phase -= std::f32::consts::TAU;
        }
        // An arpeggio's notes are each plucked: a short ramp in, then a decay to the next.
        let pluck = if stepped {
            let local = (fraction * f32::from(recipe.steps)).fract();
            (local / 0.03).min(1.0) * (1.0 - local).powf(1.2)
        } else {
            1.0
        };
        harmonic_phase += std::f32::consts::TAU * hz * recipe.harmonic_ratio / rate;
        if harmonic_phase > std::f32::consts::TAU {
            harmonic_phase -= std::f32::consts::TAU;
        }
        let tone = (phase.sin() * recipe.tone + harmonic_phase.sin() * recipe.harmonic) * pluck;

        // Sparse impulses, each a short exponential blip rather than a single sample: one
        // sample is a click at any sample rate and a blip is a crack.
        crackle_accum += crackle_step;
        if crackle_accum >= 1.0 {
            crackle_accum -= 1.0;
            crackle_level = rng.range(0.4, 1.0);
        }
        let crackle = crackle_level * rng.signed() * recipe.crackle;
        crackle_level *= 0.92;

        let tremolo = if recipe.tremolo_hz > 0.0 {
            let wobble = (std::f32::consts::TAU * recipe.tremolo_hz * time).sin();
            1.0 - recipe.tremolo_depth.clamp(0.0, 1.0) * 0.5 * (1.0 - wobble)
        } else {
            1.0
        };

        samples.push((noise + tone + crackle) * envelope(recipe, time, duration) * tremolo);
    }

    if fade > 0 {
        // Fold the tail back over the head. After this the first `fade` samples already
        // contain what the end was about to play, so the loop point is inaudible.
        for offset in 0..fade {
            let weight = offset as f32 / fade as f32;
            let tail = samples[body + offset];
            let head = samples[offset];
            samples[offset] = head * weight + tail * (1.0 - weight);
        }
        samples.truncate(body);
    }

    normalise(&mut samples);
    samples
}

/// The amplitude envelope at `time`.
///
/// A looping recipe has no envelope at all: it is a bed, and a bed that faded would fade
/// once a cycle. Its shape comes from the tremolo and from whatever is holding it, which is
/// what makes the crossfade at the seam meet two samples of the same level.
fn envelope(recipe: &Recipe, time: f32, duration: f32) -> f32 {
    if recipe.looping {
        return 1.0;
    }
    if duration <= 0.0 {
        return 0.0;
    }
    let attack = recipe.attack.max(0.0);
    if time < attack {
        return time / attack.max(f32::EPSILON);
    }
    let held = duration * recipe.sustain.clamp(0.0, 1.0);
    if time < attack + held {
        return 1.0;
    }
    let decay = (duration - attack - held).max(f32::EPSILON);
    let progress = ((time - attack - held) / decay).clamp(0.0, 1.0);
    (1.0 - progress).powf(recipe.decay_shape.max(0.1))
}

/// Scales `samples` so the loudest one sits at [`NORMALISED_PEAK`].
fn normalise(samples: &mut [f32]) {
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak <= f32::EPSILON {
        return;
    }
    let scale = NORMALISED_PEAK / peak;
    for sample in samples.iter_mut() {
        *sample *= scale;
    }
}

/// The frequency of the arpeggio note playing at `fraction`, snapped to a semitone.
fn stepped_hz(recipe: &Recipe, fraction: f32) -> f32 {
    let count = f32::from(recipe.steps.max(2));
    let note = (fraction * count).floor().min(count - 1.0);
    let hz = exp_lerp(
        recipe.tone_start_hz,
        recipe.tone_end_hz,
        note / (count - 1.0),
    );
    let semitones = (hz / recipe.tone_start_hz.max(1.0)).log2() * 12.0;
    recipe.tone_start_hz.max(1.0) * (semitones.round() / 12.0).exp2()
}

/// Linear interpolation.
fn lerp(from: f32, to: f32, fraction: f32) -> f32 {
    from + (to - from) * fraction
}

/// Interpolation in the log domain, which is how frequency is heard.
fn exp_lerp(from: f32, to: f32, fraction: f32) -> f32 {
    let from = from.max(1.0);
    let to = to.max(1.0);
    from * (to / from).powf(fraction.clamp(0.0, 1.0))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn a_rendered_sound_has_the_length_it_asked_for() {
        let recipe = Recipe::new(0.5).noise(1.0, 4_000.0, 400.0);
        let samples = render(&recipe, 48_000, 1);
        assert_eq!(samples.len(), 24_000);
    }

    #[test]
    fn every_sound_is_normalised_and_finite() {
        let recipe = Recipe::new(0.25)
            .noise(1.0, 6_000.0, 300.0)
            .tone(0.8, 200.0, 40.0)
            .crackle(0.5, 60.0);
        let samples = render(&recipe, 48_000, 7);
        let peak = samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        assert!((peak - NORMALISED_PEAK).abs() < 1e-3, "peak {peak}");
        assert!(samples.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn the_same_seed_renders_the_same_samples() {
        let recipe = Recipe::new(0.1).noise(1.0, 3_000.0, 3_000.0);
        assert_eq!(render(&recipe, 48_000, 42), render(&recipe, 48_000, 42));
        assert_ne!(render(&recipe, 48_000, 42), render(&recipe, 48_000, 43));
    }

    #[test]
    fn a_burst_decays_and_a_loop_does_not() {
        let burst = render(&Recipe::new(0.4).noise(1.0, 4_000.0, 4_000.0), 48_000, 3);
        let head = energy(&burst[..2_000]);
        let tail = energy(&burst[burst.len() - 2_000..]);
        assert!(tail < head * 0.25, "a burst must fade: {head} -> {tail}");

        let bed = render(
            &Recipe::new(1.0).noise(1.0, 2_000.0, 2_000.0).looped(),
            48_000,
            3,
        );
        let head = energy(&bed[..2_000]);
        let tail = energy(&bed[bed.len() - 2_000..]);
        assert!(
            (tail - head).abs() < head * 0.6,
            "a bed must hold its level: {head} -> {tail}"
        );
    }

    #[test]
    fn a_loop_has_no_step_at_the_seam() {
        // The join is what a click is: the last sample and the first must be neighbours,
        // not a jump across the whole range.
        let bed = render(
            &Recipe::new(0.8).noise(1.0, 1_500.0, 1_500.0).looped(),
            48_000,
            9,
        );
        let last = bed[bed.len() - 1];
        let first = bed[0];
        assert!((last - first).abs() < 0.35, "seam {last} -> {first}");
    }

    #[test]
    fn a_nonsense_duration_still_produces_a_playable_asset() {
        for duration in [0.0, -1.0, f32::NAN, f32::INFINITY, 1e9] {
            let samples = render(
                &Recipe::new(duration).noise(1.0, 1_000.0, 1_000.0),
                48_000,
                1,
            );
            assert!(!samples.is_empty());
            assert!(samples.iter().all(|sample| sample.is_finite()));
            assert!(samples.len() <= (MAX_DURATION * 48_000.0) as usize + 1);
        }
    }

    #[test]
    fn an_arpeggio_is_several_notes_and_a_sweep_is_one() {
        let notes = Recipe::new(0.6)
            .tone(1.0, 440.0, 660.0)
            .harmonic(0.4, 2.0)
            .steps(3);
        let samples = render(&notes, 48_000, 1);
        assert_eq!(samples.len(), 28_800);
        // Each plucked note starts quietly and swells to its peak: the energy of the first
        // millisecond of the second note is far below that of its middle.
        let second_start = samples.len() / 3;
        let edge = energy(&samples[second_start..second_start + 24]);
        let body = energy(&samples[second_start + 600..second_start + 1_200]);
        assert!(edge < body, "a pluck ramps in: {edge} vs {body}");
        assert!(samples.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn stepped_notes_land_on_semitones() {
        let recipe = Recipe::new(1.0).tone(1.0, 440.0, 660.0).steps(2);
        assert!((stepped_hz(&recipe, 0.1) - 440.0).abs() < 0.01);
        // 660 Hz is 7.02 semitones above 440: snapped to 7.
        let top = 440.0 * (7.0_f32 / 12.0).exp2();
        assert!((stepped_hz(&recipe, 0.9) - top).abs() < 0.01);
    }

    fn energy(samples: &[f32]) -> f32 {
        samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32
    }
}
