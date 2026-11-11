//! Sound as data, and the runtime that mixes it.
//!
//! ```text
//!            gameplay events                (engine-game, elsewhere)
//!                   │
//!                   ▼
//!            a director decides             (engine-presentation, elsewhere)
//!                   │  SoundCue
//!                   ▼
//!   ┌─────────────────────────────────┐
//!   │  AudioSystem                    │   this crate
//!   │    bank      ── SoundAsset       │
//!   │    voices    ── lifecycle, limits│
//!   │    listener  ── 3D placement     │
//!   │    mixer     ── master/SFX/UI/amb│
//!   └─────────────────────────────────┘
//!                   │  interleaved stereo frames
//!                   ▼
//!            the device callback           (engine-app + cpal, elsewhere)
//! ```
//!
//! # Why this is its own crate
//!
//! It depends on `engine-core`, and on nothing else. That absence is the design:
//!
//! * **No `engine-game`.** A sound physically cannot reach into the simulation that
//!   announced it, ask a target for its health, or consume the gameplay random source. Audio
//!   observes; it never decides. Deleting this crate would leave a game that plays
//!   identically and is silent.
//! * **No device, no `cpal`, no `windows`.** The whole runtime — the catalogue, the bank,
//!   spawning, ageing, panning, attenuation, the limiter, the priority model and the mix
//!   itself — is unit tested with no audio hardware, which is what makes the tests in this
//!   crate assertions about *samples* rather than about a machine having speakers.
//! * **No `engine-render-data` and no `engine-assets`.** A sound names a position, not a
//!   mesh, and a `SoundId`, not a file.
//!
//! ```text
//! cargo tree -p engine-audio
//! engine-audio
//! ├── engine-core
//! └── log
//! ```
//!
//! # What is here
//!
//! | Module | Owns |
//! | --- | --- |
//! | [`asset`] | The catalogue and the bank: what each sound is, and its samples |
//! | [`synth`] | The one synthesiser every sound is authored with |
//! | [`wav`] | Reading a recorded sound off disk, when there is one |
//! | [`system`] | Voices, their lifecycle, limits, priority and the mix |
//! | [`listener`] | Where the player hears from, and what that does to a sound |
//! | [`mixer`] | Master and category volumes |
//! | [`rng`] | The random source a variation is drawn from — not gameplay's |
//!
//! # Gameplay is authoritative
//!
//! Nothing in this crate can change a hit, a position or a number. Every cap in here —
//! voices, per-sound limits, minimum intervals — degrades by *dropping sound* and counting
//! the drop. If the device never opens, [`system::AudioSystem::render`] is simply never
//! called and everything else behaves exactly as it does now.

pub mod asset;
pub mod listener;
pub mod mixer;
pub mod rng;
pub mod synth;
pub mod system;
pub mod wav;

pub use asset::{
    AudioCategory, DEFAULT_MAX_DISTANCE, DEFAULT_REF_DISTANCE, PRIORITY_AMBIENT, PRIORITY_CAST,
    PRIORITY_CRITICAL, PRIORITY_DEATH, PRIORITY_FOOTSTEP, PRIORITY_IMPACT, PRIORITY_LOOP,
    PRIORITY_UI, PRIORITY_ULTIMATE, SOUND_COUNT, SOUNDS, SoundAsset, SoundBank, SoundDesc, SoundId,
};
pub use listener::{Listener, Placement, attenuate};
pub use mixer::{DEFAULT_VOLUME, Mixer, VOLUME_MAX, VOLUME_MIN};
pub use rng::Rng;
pub use synth::Recipe;
pub use system::{
    AudioStats, AudioSystem, CHANNELS, DEFAULT_MAX_VOICES, SoundCue, VoiceHandle, VoiceInfo,
};
pub use wav::WavError;
