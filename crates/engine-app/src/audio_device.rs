//! The audio device: the one place in the engine that talks to sound hardware.
//!
//! ```text
//! game thread ──▶ AudioHost::system() ──▶ Mutex<AudioSystem> ◀── cpal callback ──▶ WASAPI
//!                 (play, update, stats)                          (render)
//! ```
//!
//! # Why this is thirty lines and not three hundred
//!
//! Everything that is *about audio* — the catalogue, voices, limits, priority, panning,
//! the mix — is in `engine-audio` and is tested with no device. What is left here is
//! opening an output stream, agreeing a format with it, and copying a stereo mix into
//! whatever channel count the device asked for. That is the same division the renderer
//! draws: pipelines are ours, the queue is D3D12's.
//!
//! # A machine with no sound card still plays the game
//!
//! Every failure — no host, no device, no supported format, a stream that will not start —
//! is logged once and leaves [`AudioHost::stream`] as `None`. The runtime is still there,
//! the director still runs, cues are still counted; nothing is ever rendered. **A missing
//! device is not an error the game reports to the player**, because there is nothing they
//! can do about it in a fight and nothing about the fight changes.
//!
//! # The lock
//!
//! One mutex, held by the game thread for the frame's cues and by the callback for the mix.
//! The game thread's critical section is a few dozen field writes; the callback's is a few
//! hundred microseconds of arithmetic. A lock-free command queue would be the next step if
//! a measurement ever asked for one — `phase_audio.md` records it as deferred, and the
//! benchmark measures the cost that would justify it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use engine_audio::{AudioSystem, CHANNELS, SoundBank};

/// The rate the bank is synthesised at when no device says otherwise.
pub const FALLBACK_SAMPLE_RATE: u32 = 48_000;

/// What the opening thread reports back once the device is running.
#[derive(Debug, Clone, Copy)]
pub struct AudioOpened {
    /// Whether a device is actually playing the mix.
    pub open: bool,
    /// What the device runs at.
    pub sample_rate: u32,
    /// How many channels it takes.
    pub channels: u16,
}

/// The runtime, and the stream feeding it to the hardware.
pub struct AudioHost {
    system: Arc<Mutex<AudioSystem>>,
    /// The output stream. `None` on a machine with no usable audio device, and always `None`
    /// when the device was opened in the background: a `cpal::Stream` is not `Send`, so the
    /// thread that built it is the thread that holds it.
    ///
    /// Held only to keep the stream alive: dropping it stops playback.
    stream: Option<Stream>,
    /// What the device is running at, for the log and the readout.
    sample_rate: u32,
    /// How many channels the device takes.
    channels: u16,
    /// Whether a background thread has a device playing this system's mix.
    open_in_background: bool,
    /// How the opening thread says it is ready. `None` when nothing is opening.
    ready: Option<Receiver<AudioOpened>>,
    /// Set on drop, so the opening thread lets its stream go and exits.
    stop: Arc<AtomicBool>,
    /// The opening thread, joined on drop.
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AudioHost {
    /// Lets the opening thread go, and waits for it to put the stream down.
    ///
    /// Joined rather than detached: the stream must stop before the process does, and a
    /// thread that is only ever sleeping joins within one of its 50 ms naps.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl std::fmt::Debug for AudioHost {
    /// Hand-written because `cpal::Stream` is not `Debug`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AudioHost")
            .field("open", &self.stream.is_some())
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .finish()
    }
}

impl AudioHost {
    /// Starts opening the device on a background thread and returns a host that is silent
    /// until it does.
    ///
    /// # Why
    ///
    /// Opening the default device and synthesising the sound bank costs about 120 ms, and
    /// none of it is anything the first frame needs: no cue is played until something
    /// happens in the game. Measured against a launch of about 660 ms, it was the second
    /// largest item on the critical path and the only one with no dependency on it at all.
    ///
    /// # How the swap is invisible
    ///
    /// The `AudioSystem` behind the mutex is built *here*, empty, and handed to the thread.
    /// The thread synthesises the bank at the device's own rate and installs it into that
    /// same system, so nothing the game did meanwhile — a volume the player changed, `F8`,
    /// the listener's position — is lost, and no caller has to re-fetch anything.
    ///
    /// A cue played before the bank arrives is a cue with no samples: it is counted and
    /// mixed as silence. That is the same behaviour a machine with no sound card gets, for
    /// the fraction of a second it applies.
    #[must_use]
    pub fn open_in_background() -> Self {
        let system = Arc::new(Mutex::new(AudioSystem::with_bank(
            SoundBank::empty(FALLBACK_SAMPLE_RATE),
            FALLBACK_SAMPLE_RATE,
            engine_audio::DEFAULT_MAX_VOICES,
        )));
        let (sender, receiver) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let thread_system = Arc::clone(&system);
        let thread_stop = Arc::clone(&stop);
        let worker = std::thread::Builder::new()
            .name("audio-open".to_owned())
            .spawn(move || Self::open_on_this_thread(&thread_system, &sender, &thread_stop));

        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                // A machine that will not give us a thread still plays the game, silently:
                // the same answer as a machine with no sound card.
                log::warn!("audio: could not start the opening thread ({error}); running silently");
                None
            }
        };

        Self {
            system,
            stream: None,
            sample_rate: FALLBACK_SAMPLE_RATE,
            channels: CHANNELS as u16,
            open_in_background: false,
            ready: worker.is_some().then_some(receiver),
            stop,
            worker,
        }
    }

    /// Opens the device, installs the bank, and holds the stream until asked to stop.
    ///
    /// Runs on the opening thread and never on the game thread. It keeps the stream because
    /// `cpal::Stream` cannot be sent anywhere: dropping it stops playback, so the thread
    /// stays alive for as long as the game wants sound.
    fn open_on_this_thread(
        system: &Arc<Mutex<AudioSystem>>,
        sender: &std::sync::mpsc::Sender<AudioOpened>,
        stop: &Arc<AtomicBool>,
    ) {
        let Some((device, config, format)) = Self::choose_device() else {
            log::warn!("audio: no output device; the game will run silently");
            let _ = sender.send(AudioOpened {
                open: false,
                sample_rate: FALLBACK_SAMPLE_RATE,
                channels: CHANNELS as u16,
            });
            return;
        };
        let sample_rate = config.sample_rate;
        let channels = config.channels;

        let started = std::time::Instant::now();
        let bank = SoundBank::synthesised(sample_rate);
        log::info!(
            "audio: {} sound(s), {:.1} MiB synthesised in {:.1} ms at {sample_rate} Hz, off the \
             critical path",
            engine_audio::SOUND_COUNT,
            bank.bytes() as f64 / (1024.0 * 1024.0),
            started.elapsed().as_secs_f64() * 1e3,
        );
        match system.lock() {
            Ok(mut system) => system.install_bank(bank, sample_rate),
            Err(_) => {
                // The game thread panicked while holding the lock. There is nothing to play
                // for any more.
                log::warn!("audio: the runtime's lock is poisoned; the device is not opened");
                return;
            }
        }

        let stream = Self::build_stream(&device, &config, format, Arc::clone(system));
        let open = stream.is_some();
        if open {
            log::info!(
                "audio: {} at {sample_rate} Hz, {channels} channel(s), {format}",
                device
                    .description()
                    .map(|description| description.name().to_owned())
                    .unwrap_or_else(|_| "output".to_owned()),
            );
        }
        let _ = sender.send(AudioOpened {
            open,
            sample_rate,
            channels,
        });

        // The stream lives here or it does not live at all. Parked in short sleeps rather
        // than a condition variable: this thread wakes once, at shutdown, and a 50 ms delay
        // in noticing that is 50 ms nobody is waiting on.
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(stream);
    }

    /// Takes the opening thread's report, if it has one yet.
    ///
    /// Returns `Some` exactly once per successful open, which is the moment the application
    /// applies the player's volumes to the runtime that just gained a bank.
    pub fn poll_opened(&mut self) -> Option<AudioOpened> {
        let receiver = self.ready.as_ref()?;
        match receiver.try_recv() {
            Ok(opened) => {
                self.sample_rate = opened.sample_rate;
                self.channels = opened.channels;
                self.open_in_background = opened.open;
                self.ready = None;
                Some(opened)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.ready = None;
                None
            }
        }
    }

    /// A host with a runtime and no device at all.
    ///
    /// What `--no-audio` builds, and what a machine with no sound card gets. The runtime is
    /// real — cues are accepted, limited and counted — so "audio off" and "audio on" differ
    /// by the mix and nothing else.
    #[must_use]
    pub fn silent(sample_rate: u32) -> Self {
        Self {
            system: Arc::new(Mutex::new(AudioSystem::with_bank(
                SoundBank::empty(sample_rate),
                sample_rate,
                engine_audio::DEFAULT_MAX_VOICES,
            ))),
            stream: None,
            sample_rate,
            channels: CHANNELS as u16,
            open_in_background: false,
            ready: None,
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    /// The runtime, for the frame's cues.
    #[must_use]
    pub fn system(&self) -> &Arc<Mutex<AudioSystem>> {
        &self.system
    }

    /// Whether a device is actually playing what the runtime mixes.
    ///
    /// True for a stream this host holds and for one an opening thread holds on its behalf.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.stream.is_some() || self.open_in_background
    }

    /// The default device and the format it prefers.
    fn choose_device() -> Option<(cpal::Device, StreamConfig, SampleFormat)> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let supported = match device.default_output_config() {
            Ok(config) => config,
            Err(error) => {
                log::warn!("audio: the default device reported no usable format ({error})");
                return None;
            }
        };
        let format = supported.sample_format();
        Some((device, supported.into(), format))
    }

    /// Builds and starts the output stream, or `None` if the device refused.
    fn build_stream(
        device: &cpal::Device,
        config: &StreamConfig,
        format: SampleFormat,
        system: Arc<Mutex<AudioSystem>>,
    ) -> Option<Stream> {
        let channels = config.channels as usize;
        let on_error = |error| log::warn!("audio: the output stream failed ({error})");

        // One scratch buffer per stream, grown once to whatever the device's largest block
        // turns out to be and never freed. The callback allocates nothing after that.
        let mut scratch: Vec<f32> = Vec::new();

        let stream = match format {
            SampleFormat::F32 => device.build_output_stream(
                *config,
                move |output: &mut [f32], _| {
                    let frames = output.len() / channels.max(1);
                    let mix = mix_into(&system, &mut scratch, frames);
                    spread(mix, output, channels);
                },
                on_error,
                None,
            ),
            SampleFormat::I16 => device.build_output_stream(
                *config,
                move |output: &mut [i16], _| {
                    let frames = output.len() / channels.max(1);
                    let mix = mix_into(&system, &mut scratch, frames);
                    let mut samples = Vec::new();
                    samples.resize(output.len(), 0.0_f32);
                    spread(mix, &mut samples, channels);
                    for (target, source) in output.iter_mut().zip(samples) {
                        *target = (source.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                    }
                },
                on_error,
                None,
            ),
            other => {
                log::warn!("audio: sample format {other} is not supported; running silently");
                return None;
            }
        };

        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                log::warn!("audio: the output stream could not be built ({error})");
                return None;
            }
        };
        if let Err(error) = stream.play() {
            log::warn!("audio: the output stream could not be started ({error})");
            return None;
        }
        Some(stream)
    }
}

/// Mixes `frames` stereo frames into `scratch` and returns them.
///
/// A poisoned lock — which means a panic happened while the game thread held it — leaves the
/// buffer silent rather than propagating the panic into the audio thread, where it would
/// take the device down with it.
fn mix_into<'a>(
    system: &Mutex<AudioSystem>,
    scratch: &'a mut Vec<f32>,
    frames: usize,
) -> &'a [f32] {
    let wanted = frames * CHANNELS;
    if scratch.len() < wanted {
        scratch.resize(wanted, 0.0);
    }
    let block = &mut scratch[..wanted];
    match system.lock() {
        Ok(mut system) => system.render(block),
        Err(_) => block.fill(0.0),
    }
    block
}

/// Copies a stereo mix into a device buffer of `channels` channels.
///
/// Mono devices get the average of the two — which is what a mono speaker would have done to
/// them anyway — and anything wider gets the pair in its first two channels with the rest
/// left silent, because this engine has no surround panning and inventing one from a stereo
/// mix would be putting the same sound in five places.
fn spread(mix: &[f32], output: &mut [f32], channels: usize) {
    output.fill(0.0);
    if channels == 0 {
        return;
    }
    for (frame, samples) in output.chunks_mut(channels).enumerate() {
        let left = mix.get(frame * CHANNELS).copied().unwrap_or(0.0);
        let right = mix.get(frame * CHANNELS + 1).copied().unwrap_or(0.0);
        match channels {
            1 => samples[0] = (left + right) * 0.5,
            _ => {
                samples[0] = left;
                samples[1] = right;
            }
        }
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
    fn a_stereo_mix_reaches_a_stereo_device_unchanged() {
        let mix = [0.5, -0.5, 0.25, -0.25];
        let mut output = [0.0; 4];
        spread(&mix, &mut output, 2);
        assert_eq!(output, mix);
    }

    #[test]
    fn a_mono_device_gets_the_average() {
        let mix = [1.0, 0.0, 0.5, 0.5];
        let mut output = [0.0; 2];
        spread(&mix, &mut output, 1);
        assert_eq!(output, [0.5, 0.5]);
    }

    #[test]
    fn a_surround_device_gets_the_pair_and_silence() {
        let mix = [1.0, -1.0];
        let mut output = [9.0; 6];
        spread(&mix, &mut output, 6);
        assert_eq!(output, [1.0, -1.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_device_that_asks_for_more_than_was_mixed_gets_silence_rather_than_stale_samples() {
        let mix = [1.0, 1.0];
        let mut output = [9.0; 8];
        spread(&mix, &mut output, 2);
        assert_eq!(output[2..], [0.0; 6]);
    }

    #[test]
    fn a_silent_host_still_runs_the_whole_runtime() {
        // What a machine with no sound card gets: the game, without the sound.
        let host = AudioHost::silent(48_000);
        assert!(!host.is_open());
        let mut system = host.system().lock().expect("a fresh lock");
        assert!(
            system
                .play(engine_audio::SoundCue::ui(engine_audio::SoundId::UiClick))
                .is_none()
        );
        assert!(system.is_idle());
    }
}
