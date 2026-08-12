//! Voices, their lifecycle, and the mix.
//!
//! ```text
//!   SoundCue ──▶ play() ──▶ a voice in a fixed pool
//!                              │  update(dt)   position, listener, retirement
//!                              │  render(out)  samples into interleaved stereo
//!                              ▼
//!                        the device's buffer
//! ```
//!
//! # A fixed pool, and what happens when it is full
//!
//! [`DEFAULT_MAX_VOICES`] voices exist from construction and are never allocated again. A
//! frame of combat that asks for more than that gets the *loudest, most important* ones,
//! decided in this order:
//!
//! 1. **Interval.** A sound with a `min_interval` that started too recently is refused.
//!    This is what stops fifty simultaneous hits from being fifty voices.
//! 2. **Per-sound cap.** A sound with a `max_voices` already at its cap is refused. This is
//!    what stops fifty burning targets from being fifty burning beds.
//! 3. **Priority.** With the pool full, the lowest-priority voice playing is evicted if the
//!    new sound outranks it; otherwise the new sound is refused. An Ultimate is never
//!    displaced by a footstep, and a footstep never displaces a critical strike.
//!
//! Every refusal is counted. Nothing is ever silently dropped — the counters are in
//! [`AudioStats`], on the overlay, and in the periodic log line.
//!
//! # Two threads, one lock
//!
//! [`AudioSystem::render`] is called from the device callback and everything else from the
//! game thread, so in `engine-app` the system lives behind a mutex. The game thread's
//! critical sections are a handful of field writes; the callback's is the mix. This is the
//! simple correct arrangement, and a lock-free command queue is recorded as deferred work
//! rather than pretended at. Nothing in *this* crate knows about a thread: a test calls
//! `play`, `update` and `render` in a row and asserts on the samples.
//!
//! # Gameplay cannot reach this
//!
//! There is no path from a voice back into the simulation. A voice holds a position, a
//! sound id and a cursor; it cannot name an ability, a target, or a hit point.

use std::sync::Arc;

use engine_core::math::Vec3;

use crate::asset::{AudioCategory, SOUND_COUNT, SoundAsset, SoundBank, SoundId};
use crate::listener::{Listener, Placement};
use crate::mixer::Mixer;
use crate::rng::Rng;

/// How many voices exist at once by default.
///
/// Sixty-four is far more than a fight ever wants after the per-sound limits have had their
/// say — the stress test peaks in the twenties — and it is small enough that walking the
/// whole pool per frame is a few hundred nanoseconds.
pub const DEFAULT_MAX_VOICES: usize = 64;

/// The two channels every mix is written in.
pub const CHANNELS: usize = 2;

/// How long a gain change takes to arrive, in seconds.
///
/// Jumping a gain between two blocks is a click; a few milliseconds of ramp is inaudible as
/// a movement and audible as the absence of a click.
const GAIN_SMOOTHING: f32 = 0.008;

/// How long a stopped voice takes to fade out, in seconds.
const RELEASE: f32 = 0.02;

/// Above this the mix is compressed rather than clipped.
const SOFT_KNEE: f32 = 0.8;

/// A voice, while it lasts.
///
/// Generational: a handle to a voice that has ended names a slot whose generation has moved
/// on, so a director holding a stale handle cannot stop somebody else's sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceHandle {
    index: u32,
    generation: u32,
}

impl VoiceHandle {
    /// Its slot in the pool.
    #[must_use]
    pub fn index(self) -> usize {
        self.index as usize
    }

    /// Which use of that slot it names.
    #[must_use]
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// A request to play something.
///
/// Built with [`SoundCue::at`] for a sound in the world or [`SoundCue::ui`] for one that is
/// not, then adjusted with the builders. Everything a cue does not say comes from the
/// sound's row in the catalogue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundCue {
    /// What to play.
    pub sound: SoundId,
    /// Where it is, or `None` for a sound with no place — the interface, or a global cue.
    pub position: Option<Vec3>,
    /// An anchor key this sound follows, resolved by the caller in [`AudioSystem::update`].
    ///
    /// The same mechanism `engine-vfx` uses to pin an effect to a character: audio cannot
    /// name an entity, so the caller supplies a key and a function that turns keys into
    /// positions.
    pub anchor: Option<u32>,
    /// An extra gain on top of the catalogue's, for "this one is bigger than usual".
    pub gain: f32,
    /// An extra pitch multiplier, for the same reason.
    pub pitch: f32,
    /// Whether to loop, overriding the catalogue.
    pub looping: Option<bool>,
}

impl SoundCue {
    /// `sound`, in the world at `position`.
    #[must_use]
    pub fn at(sound: SoundId, position: Vec3) -> Self {
        Self {
            sound,
            position: Some(position),
            anchor: None,
            gain: 1.0,
            pitch: 1.0,
            looping: None,
        }
    }

    /// `sound`, with no position: never attenuated, never panned.
    #[must_use]
    pub fn ui(sound: SoundId) -> Self {
        Self {
            sound,
            position: None,
            anchor: None,
            gain: 1.0,
            pitch: 1.0,
            looping: None,
        }
    }

    /// Following whatever `anchor` resolves to each frame.
    #[must_use]
    pub fn anchored_to(mut self, anchor: u32) -> Self {
        self.anchor = Some(anchor);
        self
    }

    /// Louder or quieter than the catalogue says.
    #[must_use]
    pub fn with_gain(mut self, gain: f32) -> Self {
        self.gain = gain;
        self
    }

    /// Higher or lower than the catalogue says.
    #[must_use]
    pub fn with_pitch(mut self, pitch: f32) -> Self {
        self.pitch = pitch;
        self
    }

    /// Looping, whatever the catalogue says.
    #[must_use]
    pub fn looped(mut self, looping: bool) -> Self {
        self.looping = Some(looping);
        self
    }
}

/// What one voice is doing, for a debug readout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceInfo {
    /// Which sound.
    pub sound: SoundId,
    /// Which bus.
    pub category: AudioCategory,
    /// How hard it is to displace.
    pub priority: u8,
    /// The gain it is currently being mixed at, both channels included.
    pub level: f32,
    /// Where it is, if it is anywhere.
    pub position: Option<Vec3>,
    /// How far it is from the listener, or zero for a sound with no place.
    pub distance: f32,
    /// Whether it repeats.
    pub looping: bool,
    /// How far through the sample it is, 0 to 1. A loop wraps.
    pub progress: f32,
}

/// What the runtime is holding, and everything it has refused.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AudioStats {
    /// Voices playing right now.
    pub active: usize,
    /// The most that have ever played at once.
    pub peak_active: usize,
    /// How many there is room for.
    pub capacity: usize,
    /// Voices started over the run.
    pub started: u64,
    /// Refused because the same sound started too recently.
    pub suppressed_interval: u64,
    /// Refused because that sound was already at its own cap.
    pub suppressed_limit: u64,
    /// Refused because the pool was full and nothing was less important.
    pub dropped: u64,
    /// Voices ended early to make room for something more important.
    pub stolen: u64,
    /// Cues for a sound with no samples.
    pub missing: u64,
    /// Output samples that needed the limiter.
    pub limited: u64,
    /// Frames mixed over the run.
    pub frames: u64,
}

/// One playing sound.
#[derive(Debug, Clone)]
struct Voice {
    sound: SoundId,
    asset: Arc<SoundAsset>,
    generation: u32,
    active: bool,
    /// Where in the asset the read head is, in samples.
    cursor: f64,
    /// How far it moves per output frame — the resampling ratio and the pitch.
    step: f64,
    /// The voice's own gain: catalogue, cue and variation jitter, before the mixer.
    gain: f32,
    /// Where it is going, per channel, before the voice gain.
    target: Placement,
    /// Where it is now, ramped towards `target`.
    left: f32,
    right: f32,
    /// Multiplies everything while a stopped voice fades out.
    release: f32,
    stopping: bool,
    category: AudioCategory,
    priority: u8,
    looping: bool,
    position: Option<Vec3>,
    anchor: Option<u32>,
    /// When it started, in seconds since the system was created.
    started_at: f64,
}

impl Voice {
    /// A slot holding nothing.
    fn empty(silence: &Arc<SoundAsset>) -> Self {
        Self {
            sound: SoundId::UiClick,
            asset: Arc::clone(silence),
            generation: 0,
            active: false,
            cursor: 0.0,
            step: 1.0,
            gain: 0.0,
            target: Placement::SILENT,
            left: 0.0,
            right: 0.0,
            release: 1.0,
            stopping: false,
            category: AudioCategory::Sfx,
            priority: 0,
            looping: false,
            position: None,
            anchor: None,
            started_at: 0.0,
        }
    }
}

/// The audio runtime: a bank, a pool of voices, a mixer and a listener.
#[derive(Debug)]
pub struct AudioSystem {
    bank: SoundBank,
    voices: Vec<Voice>,
    mixer: Mixer,
    listener: Listener,
    rng: Rng,
    sample_rate: u32,
    /// Seconds since construction, advanced by [`AudioSystem::update`].
    elapsed: f64,
    /// When each sound last started, for its minimum interval.
    last_start: [f64; SOUND_COUNT],
    /// Whether a sound with no samples has already been complained about.
    warned_missing: [bool; SOUND_COUNT],
    stats: AudioStats,
    enabled: bool,
}

impl AudioSystem {
    /// A system with a synthesised bank at `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self::with_bank(
            SoundBank::synthesised(sample_rate),
            sample_rate,
            DEFAULT_MAX_VOICES,
        )
    }

    /// A system playing `bank` at `sample_rate` with room for `max_voices`.
    #[must_use]
    pub fn with_bank(bank: SoundBank, sample_rate: u32, max_voices: usize) -> Self {
        let sample_rate = if sample_rate == 0 {
            48_000
        } else {
            sample_rate
        };
        let silence = Arc::new(SoundAsset::new(vec![0.0], sample_rate));
        let voices = vec![Voice::empty(&silence); max_voices.max(1)];
        Self {
            bank,
            stats: AudioStats {
                capacity: voices.len(),
                ..AudioStats::default()
            },
            voices,
            mixer: Mixer::new(),
            listener: Listener::default(),
            rng: Rng::default(),
            sample_rate,
            elapsed: 0.0,
            last_start: [f64::NEG_INFINITY; SOUND_COUNT],
            warned_missing: [false; SOUND_COUNT],
            enabled: true,
        }
    }

    /// The rate the mix is produced at.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The bank the voices read from.
    #[must_use]
    pub fn bank(&self) -> &SoundBank {
        &self.bank
    }

    /// The bank, to load overrides into after construction.
    pub fn bank_mut(&mut self) -> &mut SoundBank {
        &mut self.bank
    }

    /// The mixer.
    #[must_use]
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// The mixer, to change a volume.
    pub fn mixer_mut(&mut self) -> &mut Mixer {
        &mut self.mixer
    }

    /// Where the player is listening from.
    #[must_use]
    pub fn listener(&self) -> Listener {
        self.listener
    }

    /// Moves the ear. Called once a frame with the gameplay camera's basis.
    pub fn set_listener(&mut self, listener: Listener) {
        self.listener = listener;
    }

    /// Whether new sounds are accepted.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Turns the whole system on or off.
    ///
    /// Off refuses new cues and ends everything playing, so "with audio" and "without audio"
    /// are two measurable states of one build rather than two builds. Unlike muting, this is
    /// a *developer* switch: it stops the work, not just the sound.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.stop_all();
        }
    }

    /// Plays `cue`, or returns `None` if it was refused.
    ///
    /// Refusal is normal and is not an error: the sound was too soon after the last one of
    /// its kind, that kind was already at its cap, or the pool was full of more important
    /// things. Every case is counted in [`AudioStats`].
    pub fn play(&mut self, cue: SoundCue) -> Option<VoiceHandle> {
        if !self.enabled {
            return None;
        }
        let desc = cue.sound.desc();
        let index = cue.sound.index();

        // A sound with no samples: the bank was empty, or a file failed to load and there
        // was no synthesised fallback. The game keeps running; the log says so once.
        let variations = self.bank.variations(cue.sound);
        if variations == 0 {
            self.stats.missing += 1;
            if !self.warned_missing[index] {
                self.warned_missing[index] = true;
                log::warn!(
                    "audio: '{}' has no samples; it will be silent for this run",
                    desc.name
                );
            }
            return None;
        }

        // 1. Too soon after the last one of these.
        if desc.min_interval > 0.0
            && self.elapsed - self.last_start[index] < f64::from(desc.min_interval)
        {
            self.stats.suppressed_interval += 1;
            return None;
        }

        // 2. Already at this sound's own cap.
        if desc.max_voices > 0 {
            let playing = self
                .voices
                .iter()
                .filter(|voice| voice.active && !voice.stopping && voice.sound == cue.sound)
                .count();
            if playing >= desc.max_voices as usize {
                self.stats.suppressed_limit += 1;
                return None;
            }
        }

        // 3. A free slot, or the least important thing playing.
        let slot = match self.free_slot() {
            Some(slot) => slot,
            None => {
                let victim = self.weakest_voice()?;
                if self.voices[victim].priority >= desc.priority {
                    self.stats.dropped += 1;
                    return None;
                }
                self.stats.stolen += 1;
                victim
            }
        };

        let variation = self.rng.index(variations);
        let asset = match self.bank.get(cue.sound, variation) {
            Some(asset) => Arc::clone(asset),
            None => {
                self.stats.missing += 1;
                return None;
            }
        };

        // Variation: a little pitch and a little level, inside the bounds the catalogue set.
        // Never enough to change what the sound *is* — a critical strike that sometimes read
        // as a normal hit would be feedback made unclear by decoration.
        let pitch_jitter = 1.0 + self.rng.signed() * desc.pitch_jitter;
        let gain_jitter = 1.0 + self.rng.signed() * desc.gain_jitter;
        let pitch = (cue.pitch * pitch_jitter).clamp(0.25, 4.0);
        let gain = (desc.gain * cue.gain * gain_jitter).clamp(0.0, 4.0);

        let looping = cue.looping.unwrap_or(desc.is_looping());
        let placement = self.placement_for(cue.position, desc.ref_distance, desc.max_distance);

        let generation = self.voices[slot].generation.wrapping_add(1);
        self.voices[slot] = Voice {
            sound: cue.sound,
            step: f64::from(asset.sample_rate()) / f64::from(self.sample_rate) * f64::from(pitch),
            asset,
            generation,
            active: true,
            cursor: 0.0,
            gain,
            target: placement,
            // Started at the target rather than ramped up from zero: a hit has an attack and
            // fading into it would take the attack off.
            left: placement.left,
            right: placement.right,
            release: 1.0,
            stopping: false,
            category: desc.category,
            priority: desc.priority,
            looping,
            position: cue.position,
            anchor: cue.anchor,
            started_at: self.elapsed,
        };

        self.last_start[index] = self.elapsed;
        self.stats.started += 1;
        self.stats.active = self.count_active();
        self.stats.peak_active = self.stats.peak_active.max(self.stats.active);
        Some(VoiceHandle {
            index: slot as u32,
            generation,
        })
    }

    /// Stops a voice, fading it out over a few milliseconds.
    ///
    /// Returns whether the handle named something that was playing. A stale handle is not an
    /// error: the sound it named ended on its own, which is the common case for a one-shot.
    pub fn stop(&mut self, handle: VoiceHandle) -> bool {
        let Some(voice) = self.voice_mut(handle) else {
            return false;
        };
        if voice.stopping {
            return false;
        }
        voice.stopping = true;
        // A looping voice that is stopping must be allowed to run past its end while it
        // fades, or the fade would be interrupted by a wrap.
        true
    }

    /// Whether a handle still names a playing voice.
    #[must_use]
    pub fn is_playing(&self, handle: VoiceHandle) -> bool {
        self.voice(handle).is_some_and(|voice| !voice.stopping)
    }

    /// Moves a playing sound. Returns whether the handle named one.
    pub fn set_position(&mut self, handle: VoiceHandle, position: Vec3) -> bool {
        let Some(voice) = self.voice_mut(handle) else {
            return false;
        };
        voice.position = Some(position);
        true
    }

    /// Changes a playing sound's gain. Returns whether the handle named one.
    pub fn set_gain(&mut self, handle: VoiceHandle, gain: f32) -> bool {
        let Some(voice) = self.voice_mut(handle) else {
            return false;
        };
        voice.gain = gain.clamp(0.0, 4.0);
        true
    }

    /// Changes a playing sound's pitch. Returns whether the handle named one.
    pub fn set_pitch(&mut self, handle: VoiceHandle, pitch: f32) -> bool {
        let rate = self.sample_rate;
        let Some(voice) = self.voice_mut(handle) else {
            return false;
        };
        voice.step = f64::from(voice.asset.sample_rate()) / f64::from(rate)
            * f64::from(pitch.clamp(0.25, 4.0));
        true
    }

    /// Ends everything immediately, with no fade. What a scene change uses.
    pub fn stop_all(&mut self) {
        for voice in &mut self.voices {
            voice.active = false;
            voice.stopping = false;
        }
        self.stats.active = 0;
    }

    /// Fades out everything on one bus.
    pub fn stop_category(&mut self, category: AudioCategory) {
        for voice in &mut self.voices {
            if voice.active && voice.category == category {
                voice.stopping = true;
            }
        }
    }

    /// Advances time, follows anchors, and retires what has finished.
    ///
    /// `anchor_of` turns a key from [`SoundCue::anchored_to`] into a position, or `None` if
    /// whatever it named is gone — in which case the voice is stopped, which is how a burning
    /// target that died stops burning audibly.
    pub fn update(&mut self, seconds: f32, mut anchor_of: impl FnMut(u32) -> Option<Vec3>) {
        let seconds = if seconds.is_finite() {
            seconds.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.elapsed += f64::from(seconds);

        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            if let Some(key) = voice.anchor {
                match anchor_of(key) {
                    Some(position) => voice.position = Some(position),
                    None => voice.stopping = true,
                }
            }
            let desc = voice.sound.desc();
            voice.target = match voice.position {
                Some(position) => {
                    self.listener
                        .place(position, desc.ref_distance, desc.max_distance)
                }
                None => Placement::CENTRED,
            };
        }

        self.stats.active = self.count_active();
    }

    /// Mixes the next `output.len() / 2` frames into `output`, interleaved stereo.
    ///
    /// This is the function the device callback calls, and the one a test calls to assert
    /// that a sound was actually audible. It overwrites `output` rather than adding to it.
    ///
    /// Called from the audio thread. It reads the mixer and the voices and writes the
    /// voices' cursors; it allocates nothing.
    pub fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let frames = output.len() / CHANNELS;
        if frames == 0 {
            return;
        }

        let smoothing = (1.0 / (GAIN_SMOOTHING * self.sample_rate as f32)).clamp(0.0, 1.0);
        let release_step = 1.0 / (RELEASE * self.sample_rate as f32).max(1.0);

        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            let bus = self.mixer.gain(voice.category);
            let length = voice.asset.len() as f64;

            for frame in 0..frames {
                // The ramp towards the placement the last update computed. Per sample rather
                // than per block so a fast-moving sound does not step.
                voice.left += (voice.target.left - voice.left) * smoothing;
                voice.right += (voice.target.right - voice.right) * smoothing;
                if voice.stopping {
                    voice.release -= release_step;
                    if voice.release <= 0.0 {
                        voice.active = false;
                        voice.stopping = false;
                        voice.release = 1.0;
                        break;
                    }
                }

                let sample = voice.asset.sample_at(voice.cursor) * voice.gain * bus * voice.release;
                let base = frame * CHANNELS;
                output[base] += sample * voice.left;
                output[base + 1] += sample * voice.right;

                voice.cursor += voice.step;
                if voice.cursor >= length {
                    if voice.looping && !voice.stopping {
                        // Wrapped rather than reset, so a loop does not gain a sample of
                        // drift every time round.
                        voice.cursor -= length;
                    } else {
                        voice.active = false;
                        voice.stopping = false;
                        voice.release = 1.0;
                        break;
                    }
                }
            }
        }

        // One soft-knee limiter across the sum. Combat can ask for a dozen voices at once
        // and the honest answer is to compress the peaks rather than to clip them, which is
        // audible as distortion exactly when the most is happening.
        let mut limited = 0_u64;
        for sample in output.iter_mut() {
            let magnitude = sample.abs();
            if magnitude > SOFT_KNEE {
                let over = magnitude - SOFT_KNEE;
                let compressed =
                    SOFT_KNEE + (1.0 - SOFT_KNEE) * (over / (over + (1.0 - SOFT_KNEE)));
                *sample = sample.signum() * compressed;
                limited += 1;
            }
        }
        self.stats.limited += limited;
        self.stats.frames += frames as u64;
    }

    /// What the runtime is holding.
    #[must_use]
    pub fn stats(&self) -> AudioStats {
        AudioStats {
            active: self.count_active(),
            ..self.stats
        }
    }

    /// How many voices are playing.
    #[must_use]
    pub fn active(&self) -> usize {
        self.count_active()
    }

    /// Whether nothing is playing.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.count_active() == 0
    }

    /// How many voices of one sound are playing. What the limiter's tests assert on.
    #[must_use]
    pub fn playing(&self, sound: SoundId) -> usize {
        self.voices
            .iter()
            .filter(|voice| voice.active && voice.sound == sound)
            .count()
    }

    /// Every playing voice, for a debug readout.
    ///
    /// Ordered by priority, loudest first within a priority, so the head of the list is what
    /// the player is actually hearing.
    #[must_use]
    pub fn voices(&self) -> Vec<VoiceInfo> {
        let mut infos: Vec<VoiceInfo> = self
            .voices
            .iter()
            .filter(|voice| voice.active)
            .map(|voice| VoiceInfo {
                sound: voice.sound,
                category: voice.category,
                priority: voice.priority,
                level: voice.gain * voice.left.max(voice.right) * self.mixer.gain(voice.category),
                position: voice.position,
                distance: if voice.target.distance.is_finite() {
                    voice.target.distance
                } else {
                    0.0
                },
                looping: voice.looping,
                progress: (voice.cursor / voice.asset.len().max(1) as f64) as f32,
            })
            .collect();
        infos.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then(right.level.total_cmp(&left.level))
        });
        infos
    }

    /// Forgets every counter but keeps what is playing. What a benchmark run starts with.
    pub fn reset_stats(&mut self) {
        let capacity = self.stats.capacity;
        self.stats = AudioStats {
            capacity,
            active: self.count_active(),
            ..AudioStats::default()
        };
    }

    /// Where a sound at `position` lands right now.
    fn placement_for(
        &self,
        position: Option<Vec3>,
        ref_distance: f32,
        max_distance: f32,
    ) -> Placement {
        match position {
            Some(position) => self.listener.place(position, ref_distance, max_distance),
            None => Placement::CENTRED,
        }
    }

    /// The first slot holding nothing.
    fn free_slot(&self) -> Option<usize> {
        self.voices.iter().position(|voice| !voice.active)
    }

    /// The playing voice that would be missed least: lowest priority, then quietest, then
    /// oldest.
    fn weakest_voice(&self) -> Option<usize> {
        self.voices
            .iter()
            .enumerate()
            .filter(|(_, voice)| voice.active)
            .min_by(|(_, left), (_, right)| {
                left.priority
                    .cmp(&right.priority)
                    .then(left.level().total_cmp(&right.level()))
                    .then(left.started_at.total_cmp(&right.started_at))
            })
            .map(|(index, _)| index)
    }

    /// How many voices are playing.
    fn count_active(&self) -> usize {
        self.voices.iter().filter(|voice| voice.active).count()
    }

    /// The voice a handle names, if it is still that voice.
    fn voice(&self, handle: VoiceHandle) -> Option<&Voice> {
        self.voices
            .get(handle.index())
            .filter(|voice| voice.active && voice.generation == handle.generation)
    }

    /// The voice a handle names, mutably.
    fn voice_mut(&mut self, handle: VoiceHandle) -> Option<&mut Voice> {
        self.voices
            .get_mut(handle.index())
            .filter(|voice| voice.active && voice.generation == handle.generation)
    }
}

impl Voice {
    /// How loud this voice currently is, for eviction order.
    fn level(&self) -> f32 {
        self.gain * self.left.max(self.right)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// A system with a small synthesised bank: the real catalogue, a tenth of the samples.
    fn system() -> AudioSystem {
        AudioSystem::with_bank(SoundBank::synthesised(8_000), 8_000, DEFAULT_MAX_VOICES)
    }

    /// The loudest sample in one second of mix.
    fn peak(system: &mut AudioSystem) -> f32 {
        let mut output = vec![0.0; 8_000 * CHANNELS];
        system.render(&mut output);
        output.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()))
    }

    #[test]
    fn a_played_sound_is_audible_and_then_ends() {
        let mut system = system();
        let handle = system
            .play(SoundCue::ui(SoundId::UiClick))
            .expect("the pool is empty");
        assert!(system.is_playing(handle));
        assert!(
            peak(&mut system) > 0.0,
            "silence came out of a played sound"
        );
        // UiClick is under a tenth of a second; one second of mix is well past its end.
        assert!(!system.is_playing(handle));
        assert!(system.is_idle());
    }

    #[test]
    fn a_loop_keeps_playing_until_it_is_stopped() {
        let mut system = system();
        let handle = system
            .play(SoundCue::at(SoundId::IgniteLoop, Vec3::ZERO))
            .expect("plays");
        for _ in 0..4 {
            assert!(peak(&mut system) > 0.0);
            assert!(system.is_playing(handle));
        }
        assert!(system.stop(handle));
        assert!(!system.is_playing(handle));
        // The release is milliseconds; one block finishes it.
        peak(&mut system);
        assert!(system.is_idle(), "a stopped loop must actually stop");
    }

    #[test]
    fn a_stale_handle_cannot_stop_somebody_elses_sound() {
        let mut system = system();
        let first = system.play(SoundCue::ui(SoundId::UiClick)).expect("plays");
        peak(&mut system);
        assert!(!system.is_playing(first));
        let second = system.play(SoundCue::ui(SoundId::UiSelect)).expect("plays");
        assert!(!system.stop(first), "the old handle names nothing");
        assert!(system.is_playing(second));
        assert_ne!(first.generation(), second.generation());
    }

    #[test]
    fn a_missing_sound_is_silent_and_counted_rather_than_fatal() {
        let mut system = AudioSystem::with_bank(SoundBank::empty(8_000), 8_000, 8);
        assert!(system.play(SoundCue::ui(SoundId::UiClick)).is_none());
        assert!(
            system
                .play(SoundCue::at(SoundId::HitNormal, Vec3::ZERO))
                .is_none()
        );
        assert_eq!(system.stats().missing, 2);
        assert_eq!(peak(&mut system), 0.0);
        assert!(system.is_idle());
    }

    #[test]
    fn the_minimum_interval_thins_a_burst_of_the_same_sound() {
        let mut system = system();
        // Fifty targets hit on one tick: hit_normal's interval is 35 ms, so one gets through
        // and the rest are counted rather than played.
        let played = (0..50)
            .filter(|_| {
                system
                    .play(SoundCue::at(SoundId::HitNormal, Vec3::ZERO))
                    .is_some()
            })
            .count();
        assert_eq!(played, 1, "a tick of hits is one hit sound");
        assert_eq!(system.stats().suppressed_interval, 49);

        // Time passes, and the next one is allowed.
        system.update(0.05, |_| None);
        assert!(
            system
                .play(SoundCue::at(SoundId::HitNormal, Vec3::ZERO))
                .is_some()
        );
    }

    #[test]
    fn a_per_sound_cap_stops_a_wall_of_identical_loops() {
        let mut system = system();
        let cap = SoundId::IgniteLoop.desc().max_voices as usize;
        let mut handles = Vec::new();
        for index in 0..50 {
            let position = Vec3::new(index as f32, 0.0, 0.0);
            if let Some(handle) = system.play(SoundCue::at(SoundId::IgniteLoop, position)) {
                handles.push(handle);
            }
        }
        assert_eq!(handles.len(), cap, "fifty burning targets, {cap} beds");
        assert_eq!(system.playing(SoundId::IgniteLoop), cap);
        assert!(system.stats().suppressed_limit >= 46);
    }

    #[test]
    fn priority_decides_who_keeps_a_voice_when_the_pool_is_full() {
        // Three voices, which is also the footstep's own cap: the pool fills with the least
        // important thing there is.
        let mut system = AudioSystem::with_bank(SoundBank::synthesised(8_000), 8_000, 3);
        for index in 0..3 {
            system
                .play(SoundCue::at(
                    SoundId::Footstep,
                    Vec3::new(index as f32, 0.0, 0.0),
                ))
                .expect("a footstep fits");
            system.update(0.2, |_| None);
        }
        assert_eq!(system.active(), 3);

        // The Ultimate takes a voice from a footstep.
        assert!(
            system
                .play(SoundCue::at(SoundId::CataclysmImpact, Vec3::ZERO))
                .is_some()
        );
        assert_eq!(system.stats().stolen, 1);
        assert_eq!(system.playing(SoundId::Footstep), 2);

        // Two critical strikes take the two that are left, because a critical outranks a
        // footstep too.
        for _ in 0..2 {
            system.update(0.2, |_| None);
            assert!(
                system
                    .play(SoundCue::at(SoundId::HitCritical, Vec3::ZERO))
                    .is_some()
            );
        }
        assert_eq!(system.playing(SoundId::Footstep), 0);
        assert_eq!(system.stats().stolen, 3);

        // And now nothing in the pool is less important than a footstep, so the footstep is
        // the one that is refused.
        system.update(0.5, |_| None);
        assert!(
            system
                .play(SoundCue::at(SoundId::Footstep, Vec3::ZERO))
                .is_none()
        );
        assert_eq!(system.stats().dropped, 1);
    }

    #[test]
    fn a_distant_sound_is_quieter_than_a_near_one() {
        let mut near = system();
        near.play(SoundCue::at(
            SoundId::MeteorImpact,
            Vec3::new(0.0, 0.0, -2.0),
        ));
        let near_peak = peak(&mut near);

        let mut far = system();
        far.play(SoundCue::at(
            SoundId::MeteorImpact,
            Vec3::new(0.0, 0.0, -40.0),
        ));
        let far_peak = peak(&mut far);

        assert!(near_peak > far_peak * 2.0, "{near_peak} vs {far_peak}");
        assert!(far_peak > 0.0, "40 m is inside the range and must be heard");
    }

    #[test]
    fn a_sound_to_one_side_comes_out_of_that_side() {
        let mut system = system();
        let right = system.listener().right() * 6.0;
        system.play(SoundCue::at(SoundId::MeteorImpact, right));
        let mut output = vec![0.0; 4_000 * CHANNELS];
        system.render(&mut output);
        let left_energy: f32 = output.iter().step_by(2).map(|s| s * s).sum();
        let right_energy: f32 = output.iter().skip(1).step_by(2).map(|s| s * s).sum();
        assert!(
            right_energy > left_energy * 2.0,
            "{left_energy} {right_energy}"
        );
    }

    #[test]
    fn the_interface_is_not_attenuated_by_where_the_player_stands() {
        let mut system = system();
        system.set_listener(Listener::new(
            Vec3::new(500.0, 0.0, 500.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 1.0, 0.0),
        ));
        system.play(SoundCue::ui(SoundId::UiClick));
        assert!(peak(&mut system) > 0.0, "UI must not be a world sound");
    }

    #[test]
    fn category_volume_changes_what_comes_out() {
        let mut full = system();
        full.play(SoundCue::ui(SoundId::UiClick));
        let loud = peak(&mut full);

        let mut turned_down = system();
        turned_down
            .mixer_mut()
            .set_category(AudioCategory::Ui, 0.25);
        turned_down.play(SoundCue::ui(SoundId::UiClick));
        let quiet = peak(&mut turned_down);
        assert!(quiet < loud * 0.5, "{quiet} vs {loud}");

        let mut silenced = system();
        silenced.mixer_mut().set_master(0.0);
        silenced.play(SoundCue::ui(SoundId::UiClick));
        assert_eq!(peak(&mut silenced), 0.0, "master zero is silence");
    }

    #[test]
    fn an_anchored_voice_follows_its_anchor_and_stops_when_it_is_gone() {
        let mut system = system();
        let handle = system
            .play(SoundCue::at(SoundId::IgniteLoop, Vec3::ZERO).anchored_to(7))
            .expect("plays");
        system.update(0.1, |key| {
            assert_eq!(key, 7);
            Some(Vec3::new(0.0, 0.0, -10.0))
        });
        let moved = system.voices().first().copied().expect("one voice");
        assert!((moved.distance - 10.0).abs() < 1e-3);

        // The target died: the anchor resolves to nothing and the bed stops.
        system.update(0.1, |_| None);
        assert!(!system.is_playing(handle));
    }

    #[test]
    fn the_mix_never_leaves_the_range_a_device_accepts() {
        let mut system = AudioSystem::with_bank(SoundBank::synthesised(8_000), 8_000, 64);
        // Everything at once, on top of the listener, at full volume.
        for _ in 0..64 {
            system.play(SoundCue::at(SoundId::PyroblastImpact, Vec3::ZERO).with_gain(4.0));
            system.update(0.2, |_| None);
        }
        let mut output = vec![0.0; 2_048 * CHANNELS];
        system.render(&mut output);
        assert!(
            output
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0),
            "the limiter let something through"
        );
        assert!(system.stats().limited > 0);
    }

    #[test]
    fn rendering_zero_frames_and_odd_buffers_is_harmless() {
        let mut system = system();
        system.play(SoundCue::ui(SoundId::UiClick));
        system.render(&mut []);
        let mut odd = vec![0.0; 3];
        system.render(&mut odd);
        assert!(odd.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn a_disabled_system_plays_nothing_and_drops_what_was_playing() {
        let mut system = system();
        system.play(SoundCue::at(SoundId::IgniteLoop, Vec3::ZERO));
        system.set_enabled(false);
        assert!(system.is_idle());
        assert!(system.play(SoundCue::ui(SoundId::UiClick)).is_none());
        assert_eq!(peak(&mut system), 0.0);
        system.set_enabled(true);
        assert!(system.play(SoundCue::ui(SoundId::UiClick)).is_some());
    }

    #[test]
    fn stats_count_every_refusal() {
        let mut system = system();
        for _ in 0..10 {
            system.play(SoundCue::at(SoundId::HitNormal, Vec3::ZERO));
        }
        let stats = system.stats();
        assert_eq!(stats.started, 1);
        assert_eq!(stats.suppressed_interval, 9);
        assert_eq!(stats.capacity, DEFAULT_MAX_VOICES);
        assert!(stats.peak_active >= 1);
    }

    #[test]
    fn a_nonsense_delta_does_not_move_time_backwards() {
        let mut system = system();
        system.play(SoundCue::at(SoundId::HitNormal, Vec3::ZERO));
        for delta in [f32::NAN, -5.0, f32::INFINITY] {
            system.update(delta, |_| None);
        }
        // Time did not jump, so the interval still holds.
        assert!(
            system
                .play(SoundCue::at(SoundId::HitNormal, Vec3::ZERO))
                .is_none()
        );
    }

    #[test]
    fn the_debug_readout_lists_the_loudest_and_most_important_first() {
        let mut system = system();
        system.play(SoundCue::at(SoundId::Footstep, Vec3::ZERO));
        system.play(SoundCue::at(SoundId::CataclysmImpact, Vec3::ZERO));
        system.update(0.0, |_| None);
        let voices = system.voices();
        assert_eq!(voices.len(), 2);
        assert_eq!(voices[0].sound, SoundId::CataclysmImpact);
        assert!(voices[0].priority > voices[1].priority);
    }
}
