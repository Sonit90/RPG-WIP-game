//! The catalogue: every sound the game can make, as a static table, and the bank that holds
//! the samples.
//!
//! ```text
//! SoundId ── an identifier gameplay-facing code says out loud
//!    │
//!    ├─ SoundDesc ── category, priority, gain, distances, limits, and the recipe
//!    │
//!    ▼
//! SoundBank ── the PCM, one or more variations per id, synthesised or read from disk
//! ```
//!
//! # Nothing outside this file names a file
//!
//! Presentation asks for [`SoundId::FireboltImpact`]. It does not know whether that is a
//! synthesised burst, a WAV beside the executable, three WAVs picked between, or nothing at
//! all. That indirection is the whole point of the milestone: adding a sound to an ability
//! is a line in a table here plus a line in the director, and **no change to gameplay**.
//!
//! # A missing sound is not an error
//!
//! [`SoundBank::get`] returns `None` and the runtime plays silence. A game that cannot find
//! its audio still runs, still fights, and still draws — it is simply quiet, and it says so
//! once in the log per id rather than every frame.
//!
//! # Where files go when there are files
//!
//! ```text
//! assets/audio/
//! ├── abilities/     casts, projectiles, the sounds an ability makes
//! ├── impacts/       hits, criticals, explosions, death
//! ├── characters/    footsteps, jump, landing
//! ├── environment/   ambience
//! └── ui/            the interface
//! ```
//!
//! A file named `<id>.wav` replaces that id's single variation; `<id>_1.wav`, `<id>_2.wav`
//! … replace its variations one for one. Anything that fails to parse is skipped with a
//! warning and the synthesised sound stays.

use std::path::Path;
use std::sync::Arc;

use crate::synth::{self, Recipe};
use crate::wav;

/// Which bus a sound is mixed on.
///
/// Three, deliberately: these are the three sliders a player expects and the three volumes
/// that mean different things. Sub-buses (abilities, character, environment) are what the
/// [`SoundDesc::priority`] and the per-id gain already express, and a fourth slider nobody
/// moves is a fourth thing to keep balanced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioCategory {
    /// Everything that happens in the world: abilities, impacts, footsteps.
    Sfx,
    /// The interface. Never positional, never attenuated.
    Ui,
    /// The bed the world sits on. Quiet, looping, always there.
    Ambient,
}

impl AudioCategory {
    /// Every category, in table order.
    pub const ALL: [Self; 3] = [Self::Sfx, Self::Ui, Self::Ambient];

    /// Its index into a per-category array.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Sfx => 0,
            Self::Ui => 1,
            Self::Ambient => 2,
        }
    }

    /// Short uppercase name, for the developer overlay's font and the log.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sfx => "SFX",
            Self::Ui => "UI",
            Self::Ambient => "AMBIENT",
        }
    }
}

/// The priority a sound nothing else competes with gets.
pub const PRIORITY_AMBIENT: u8 = 10;
/// Footsteps: the first thing that should go when a fight is loud.
pub const PRIORITY_FOOTSTEP: u8 = 20;
/// The looping beds a burning target or a running cast keeps up.
pub const PRIORITY_LOOP: u8 = 35;
/// An ability being cast.
pub const PRIORITY_CAST: u8 = 50;
/// An ability landing.
pub const PRIORITY_IMPACT: u8 = 65;
/// Something dying.
pub const PRIORITY_DEATH: u8 = 75;
/// A critical strike, which the player is listening for.
pub const PRIORITY_CRITICAL: u8 = 85;
/// The interface, which must be heard because the player just pressed it.
pub const PRIORITY_UI: u8 = 90;
/// The Ultimate. Nothing is allowed to take its voice.
pub const PRIORITY_ULTIMATE: u8 = 100;

/// Everything about a sound except its samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundDesc {
    /// Its name in the log, the overlay, and on disk.
    pub name: &'static str,
    /// The folder under `assets/audio` an override would live in.
    pub folder: &'static str,
    /// Which bus it mixes on.
    pub category: AudioCategory,
    /// How it is synthesised when there is no file.
    pub recipe: Recipe,
    /// How many variations to synthesise. At least one.
    pub variations: u8,
    /// Who wins when the voice pool is full. Higher stays.
    pub priority: u8,
    /// Its level relative to every other sound, before any mixer volume.
    pub gain: f32,
    /// The shortest time between two starts of this id, in seconds. Zero for no limit.
    pub min_interval: f32,
    /// The most voices of this id that may play at once. Zero for no limit.
    pub max_voices: u8,
    /// Within this distance a positional sound is at full volume, in metres.
    pub ref_distance: f32,
    /// Past this distance it is silent, in metres.
    pub max_distance: f32,
    /// How far the pitch may wander, as a fraction. 0.06 is about a semitone.
    pub pitch_jitter: f32,
    /// How far the gain may wander, as a fraction.
    pub gain_jitter: f32,
}

impl SoundDesc {
    /// A one-shot world sound: `name` in `folder`, at `gain`, with `priority`.
    #[must_use]
    const fn sfx(name: &'static str, folder: &'static str, recipe: Recipe, priority: u8) -> Self {
        Self {
            name,
            folder,
            category: AudioCategory::Sfx,
            recipe,
            variations: 1,
            priority,
            gain: 1.0,
            min_interval: 0.0,
            max_voices: 0,
            ref_distance: DEFAULT_REF_DISTANCE,
            max_distance: DEFAULT_MAX_DISTANCE,
            pitch_jitter: 0.05,
            gain_jitter: 0.08,
        }
    }

    /// With `count` variations synthesised from different seeds.
    #[must_use]
    const fn variations(mut self, count: u8) -> Self {
        self.variations = count;
        self
    }

    /// At `gain` relative to everything else.
    #[must_use]
    const fn gain(mut self, gain: f32) -> Self {
        self.gain = gain;
        self
    }

    /// No more than `voices` at once, and no more than one start every `interval` seconds.
    #[must_use]
    const fn limit(mut self, voices: u8, interval: f32) -> Self {
        self.max_voices = voices;
        self.min_interval = interval;
        self
    }

    /// Audible from further away than the default. Big things carry.
    #[must_use]
    const fn carries(mut self, reference: f32, max: f32) -> Self {
        self.ref_distance = reference;
        self.max_distance = max;
        self
    }

    /// On a different bus.
    #[must_use]
    const fn on(mut self, category: AudioCategory) -> Self {
        self.category = category;
        self
    }

    /// With variation bounds of its own.
    #[must_use]
    const fn jitter(mut self, pitch: f32, gain: f32) -> Self {
        self.pitch_jitter = pitch;
        self.gain_jitter = gain;
        self
    }

    /// Whether the sound repeats until it is stopped.
    #[must_use]
    pub const fn is_looping(&self) -> bool {
        self.recipe.looping
    }
}

/// Within this many metres a positional sound plays at full volume.
///
/// Roughly the length of the mage's own body plus arms: closer than this, moving does not
/// change how loud something is, which is what stops a sound sitting on the player from
/// swinging wildly as the camera breathes.
pub const DEFAULT_REF_DISTANCE: f32 = 2.5;

/// Past this many metres an ordinary sound is silent.
///
/// The class's longest range is 35 m, so a fight is inside this and a sound from the far
/// side of the arena is not.
pub const DEFAULT_MAX_DISTANCE: f32 = 55.0;

/// Every sound in the game.
///
/// Adding one means a variant on [`SoundId`], a line in [`SoundId::ALL`] and a row here.
/// Nothing else in the engine changes — least of all gameplay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundId {
    /// Fire gathering in the hand while Firebolt is being cast.
    FireboltCharge,
    /// Firebolt leaving the hand.
    FireboltCast,
    /// The bolt in the air. Loops for as long as the cosmetic bolt flies.
    FireboltTravel,
    /// The bolt arriving.
    FireboltImpact,
    /// Flame Lance's beam, while it is being channelled into a target.
    FlameLanceCast,
    /// Flame Lance landing.
    FlameLanceImpact,
    /// The cone of flame going out.
    FlameConeCast,
    /// Meteor being called.
    MeteorCast,
    /// The rock on its way down. Loops for the fall.
    MeteorIncoming,
    /// The rock arriving.
    MeteorImpact,
    /// Pyroblast gathering. Loops for the cast.
    PyroblastCast,
    /// Pyroblast landing.
    PyroblastImpact,
    /// Firestorm being laid down.
    FirestormCast,
    /// The line of fire burning. Loops for the zone's life.
    FirestormLoop,
    /// Detonate's wind-up.
    DetonateCast,
    /// Detonate's explosion.
    DetonateExplosion,
    /// Fire Mastery coming up.
    FireMasteryActivate,
    /// The aura while Fire Mastery is up.
    FireMasteryLoop,
    /// Flame Shield coming up.
    FlameShieldActivate,
    /// The shell while Flame Shield is up.
    FlameShieldLoop,
    /// Leaving somewhere by Blink.
    BlinkDepart,
    /// Arriving somewhere by Blink.
    BlinkArrive,
    /// The Ultimate gathering.
    CataclysmCast,
    /// The Ultimate landing.
    CataclysmImpact,
    /// A target catching fire.
    IgniteApply,
    /// A target burning. Loops while the burn is on it.
    IgniteLoop,
    /// A cast that did not finish.
    CastFizzle,
    /// An ordinary hit.
    HitNormal,
    /// A critical strike.
    HitCritical,
    /// A hit big enough to be felt rather than heard.
    HeavyImpact,
    /// Something dying: the player, and any enemy without a death of its own.
    Death,
    /// A skeleton falling apart.
    DeathSkeleton,
    /// A robed acolyte fading out.
    DeathAcolyte,
    /// A brute hitting the floor.
    DeathBrute,
    /// A ghoul's last screech.
    DeathGhoul,
    /// A boss going down.
    DeathBoss,
    /// An enemy winding up a quick attack. Loops for as long as the cast does.
    EnemyCastQuick,
    /// An enemy winding up a sweeping attack. Loops for as long as the cast does.
    EnemyCastSweep,
    /// An enemy winding up a heavy attack. Loops for as long as the cast does.
    EnemyCastHeavy,
    /// The warning hum over a telegraphed area, for as long as it is on the ground.
    EnemyTelegraph,
    /// A telegraphed area going off.
    EnemyTelegraphBurst,
    /// A dungeon modifier's beams lighting: a low hum that swells as they turn.
    ModifierBeamsStart,
    /// A slain monster beginning to fester: a short bubbling tick.
    ModifierPlagueArm,
    /// A festering corpse bursting: a wet burst.
    ModifierPlagueExplode,
    /// One tick of the key reroll's dice rolling: played over and over while the reveal runs.
    ModifierReroll,
    /// The reroll landing and the modifier getting stronger: a bright rising stinger.
    ModifierResult,
    /// The reroll landing and the modifier getting weaker: a falling stinger.
    ModifierResultWeak,
    /// One step.
    Footstep,
    /// The arena's wind. Loops forever.
    AmbientWind,
    /// An occasional ember somewhere in the arena.
    AmbientEmber,
    /// A button.
    UiClick,
    /// A choice being made.
    UiSelect,
    /// A screen being closed.
    UiCancel,
}

/// How many sounds there are.
pub const SOUND_COUNT: usize = 53;

impl SoundId {
    /// Every sound, in table order. The order is the order of [`SOUNDS`].
    pub const ALL: [Self; SOUND_COUNT] = [
        Self::FireboltCharge,
        Self::FireboltCast,
        Self::FireboltTravel,
        Self::FireboltImpact,
        Self::FlameLanceCast,
        Self::FlameLanceImpact,
        Self::FlameConeCast,
        Self::MeteorCast,
        Self::MeteorIncoming,
        Self::MeteorImpact,
        Self::PyroblastCast,
        Self::PyroblastImpact,
        Self::FirestormCast,
        Self::FirestormLoop,
        Self::DetonateCast,
        Self::DetonateExplosion,
        Self::FireMasteryActivate,
        Self::FireMasteryLoop,
        Self::FlameShieldActivate,
        Self::FlameShieldLoop,
        Self::BlinkDepart,
        Self::BlinkArrive,
        Self::CataclysmCast,
        Self::CataclysmImpact,
        Self::IgniteApply,
        Self::IgniteLoop,
        Self::CastFizzle,
        Self::HitNormal,
        Self::HitCritical,
        Self::HeavyImpact,
        Self::Death,
        Self::DeathSkeleton,
        Self::DeathAcolyte,
        Self::DeathBrute,
        Self::DeathGhoul,
        Self::DeathBoss,
        Self::EnemyCastQuick,
        Self::EnemyCastSweep,
        Self::EnemyCastHeavy,
        Self::EnemyTelegraph,
        Self::EnemyTelegraphBurst,
        Self::ModifierBeamsStart,
        Self::ModifierPlagueArm,
        Self::ModifierPlagueExplode,
        Self::ModifierReroll,
        Self::ModifierResult,
        Self::ModifierResultWeak,
        Self::Footstep,
        Self::AmbientWind,
        Self::AmbientEmber,
        Self::UiClick,
        Self::UiSelect,
        Self::UiCancel,
    ];

    /// Its index into [`SOUNDS`] and into any per-sound array.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Everything about it except its samples.
    #[must_use]
    pub fn desc(self) -> &'static SoundDesc {
        // The table is built in variant order and the test below asserts it, so indexing by
        // the discriminant cannot pick up the wrong row.
        &SOUNDS[self.index()]
    }

    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.desc().name
    }

    /// Whether it repeats until stopped.
    #[must_use]
    pub fn is_looping(self) -> bool {
        self.desc().is_looping()
    }
}

/// Folder names under `assets/audio`.
const ABILITIES: &str = "abilities";
const IMPACTS: &str = "impacts";
const CHARACTERS: &str = "characters";
const ENVIRONMENT: &str = "environment";
const UI: &str = "ui";

/// The catalogue.
///
/// Read this table to know what the game sounds like. Every explosion in it is the same
/// recipe at a different size and a different pitch, which is what makes Meteor, Detonate,
/// Pyroblast and the Ultimate sound like members of one family rather than four unrelated
/// noises — and what stops the bank holding four nearly identical recordings.
pub static SOUNDS: [SoundDesc; SOUND_COUNT] = [
    // ---- Firebolt: the filler, so it is short, bright and cheap ----------------------
    SoundDesc::sfx(
        "firebolt_charge",
        ABILITIES,
        Recipe::new(0.6)
            .noise(0.7, 900.0, 1_600.0)
            .tremolo(11.0, 0.3)
            .looped(),
        PRIORITY_CAST,
    )
    .gain(0.4),
    SoundDesc::sfx(
        "firebolt_cast",
        ABILITIES,
        Recipe::new(0.28)
            .noise(0.9, 2_600.0, 900.0)
            .tone(0.35, 520.0, 180.0)
            .envelope(0.005, 0.05, 2.4),
        PRIORITY_CAST,
    )
    .variations(3)
    .gain(0.55),
    SoundDesc::sfx(
        "firebolt_travel",
        ABILITIES,
        Recipe::new(0.5)
            .noise(0.8, 1_400.0, 1_100.0)
            .tremolo(9.0, 0.35)
            .looped(),
        PRIORITY_LOOP,
    )
    .gain(0.22)
    .limit(6, 0.0),
    SoundDesc::sfx(
        "firebolt_impact",
        IMPACTS,
        Recipe::new(0.35)
            .noise(1.0, 3_400.0, 400.0)
            .tone(0.6, 260.0, 70.0)
            .crackle(0.35, 90.0)
            .envelope(0.002, 0.02, 2.8),
        PRIORITY_IMPACT,
    )
    .variations(3)
    .gain(0.6)
    .limit(6, 0.03),
    // ---- Flame Lance: a beam, so the cast is the sound and the impact is a punctuation -
    SoundDesc::sfx(
        "flame_lance_cast",
        ABILITIES,
        Recipe::new(0.7)
            .noise(0.95, 1_800.0, 1_500.0)
            .tone(0.25, 150.0, 190.0)
            .tremolo(14.0, 0.25)
            .looped(),
        PRIORITY_CAST,
    )
    .gain(0.45),
    SoundDesc::sfx(
        "flame_lance_impact",
        IMPACTS,
        Recipe::new(0.45)
            .noise(1.0, 2_800.0, 320.0)
            .tone(0.7, 200.0, 55.0)
            .crackle(0.3, 70.0)
            .envelope(0.003, 0.04, 2.6),
        PRIORITY_IMPACT,
    )
    .variations(2)
    .gain(0.7)
    .limit(6, 0.03),
    // ---- Flame Cone: wide and breathy, and it is the only sweep in the class ----------
    SoundDesc::sfx(
        "flame_cone_cast",
        ABILITIES,
        Recipe::new(0.85)
            .noise(1.0, 900.0, 3_200.0)
            .crackle(0.25, 55.0)
            .envelope(0.09, 0.25, 1.6),
        PRIORITY_CAST,
    )
    .variations(2)
    .gain(0.8)
    .carries(4.0, 60.0),
    // ---- Meteor: called, falls, lands. Three sounds because it is three moments --------
    SoundDesc::sfx(
        "meteor_cast",
        ABILITIES,
        Recipe::new(0.6)
            .noise(0.6, 700.0, 2_400.0)
            .tone(0.5, 90.0, 300.0)
            .envelope(0.15, 0.1, 1.4),
        PRIORITY_CAST,
    )
    .gain(0.7),
    SoundDesc::sfx(
        "meteor_incoming",
        ABILITIES,
        Recipe::new(1.0)
            .noise(0.7, 2_200.0, 2_600.0)
            .tone(0.55, 700.0, 620.0)
            .tremolo(3.0, 0.3)
            .looped(),
        PRIORITY_LOOP,
    )
    .gain(0.5)
    .carries(6.0, 70.0)
    .limit(3, 0.0),
    SoundDesc::sfx(
        "meteor_impact",
        IMPACTS,
        Recipe::new(1.5)
            .noise(1.0, 2_600.0, 120.0)
            .tone(0.9, 130.0, 32.0)
            .crackle(0.4, 40.0)
            .envelope(0.003, 0.03, 2.2),
        PRIORITY_IMPACT,
    )
    .variations(2)
    .gain(1.0)
    .carries(8.0, 90.0)
    .limit(3, 0.05),
    // ---- Pyroblast: the long cast is a rising bed, the landing is the biggest single hit
    SoundDesc::sfx(
        "pyroblast_cast",
        ABILITIES,
        Recipe::new(1.2)
            .noise(0.8, 1_200.0, 1_600.0)
            .tone(0.45, 110.0, 150.0)
            .tremolo(6.0, 0.4)
            .looped(),
        PRIORITY_CAST,
    )
    .gain(0.5),
    SoundDesc::sfx(
        "pyroblast_impact",
        IMPACTS,
        Recipe::new(1.6)
            .noise(1.0, 3_000.0, 100.0)
            .tone(0.95, 160.0, 28.0)
            .crackle(0.45, 55.0)
            .envelope(0.002, 0.04, 2.0),
        PRIORITY_IMPACT,
    )
    .variations(2)
    .gain(1.0)
    .carries(8.0, 90.0)
    .limit(3, 0.05),
    // ---- Firestorm: laid down once, then a bed for as long as the ground burns ---------
    SoundDesc::sfx(
        "firestorm_cast",
        ABILITIES,
        Recipe::new(0.7)
            .noise(0.9, 1_600.0, 600.0)
            .crackle(0.3, 60.0)
            .envelope(0.05, 0.15, 1.8),
        PRIORITY_CAST,
    )
    .gain(0.7),
    SoundDesc::sfx(
        "firestorm_loop",
        ABILITIES,
        Recipe::new(2.0)
            .noise(0.9, 1_100.0, 900.0)
            .crackle(0.35, 24.0)
            .tremolo(2.5, 0.35)
            .looped(),
        PRIORITY_LOOP,
    )
    .gain(0.45)
    .carries(4.0, 60.0)
    .limit(4, 0.0),
    // ---- Detonate: a short intake, then the payoff -------------------------------------
    SoundDesc::sfx(
        "detonate_cast",
        ABILITIES,
        Recipe::new(0.25)
            .noise(0.7, 3_000.0, 5_000.0)
            .envelope(0.12, 0.0, 1.2),
        PRIORITY_CAST,
    )
    .gain(0.5),
    SoundDesc::sfx(
        "detonate_explosion",
        IMPACTS,
        Recipe::new(1.1)
            .noise(1.0, 3_200.0, 160.0)
            .tone(0.85, 190.0, 40.0)
            .crackle(0.4, 65.0)
            .envelope(0.002, 0.02, 2.4),
        PRIORITY_IMPACT,
    )
    .variations(3)
    .gain(0.95)
    .carries(6.0, 80.0)
    .limit(4, 0.04),
    // ---- Cooldowns: one moment, then a bed while the state is up -----------------------
    SoundDesc::sfx(
        "fire_mastery_activate",
        ABILITIES,
        Recipe::new(0.8)
            .noise(0.5, 1_400.0, 4_000.0)
            .tone(0.7, 180.0, 540.0)
            .envelope(0.05, 0.15, 1.8),
        PRIORITY_CAST,
    )
    .gain(0.75),
    SoundDesc::sfx(
        "fire_mastery_loop",
        ABILITIES,
        Recipe::new(2.0)
            .noise(0.6, 2_400.0, 2_000.0)
            .tone(0.3, 320.0, 300.0)
            .tremolo(1.6, 0.5)
            .looped(),
        PRIORITY_LOOP,
    )
    .gain(0.2),
    SoundDesc::sfx(
        "flame_shield_activate",
        ABILITIES,
        Recipe::new(0.6)
            .noise(0.8, 900.0, 2_600.0)
            .tone(0.5, 120.0, 340.0)
            .envelope(0.03, 0.1, 2.0),
        PRIORITY_CAST,
    )
    .gain(0.8),
    SoundDesc::sfx(
        "flame_shield_loop",
        ABILITIES,
        Recipe::new(1.8)
            .noise(0.7, 800.0, 700.0)
            .tremolo(3.5, 0.45)
            .looped(),
        PRIORITY_LOOP,
    )
    .gain(0.25),
    // ---- Blink: two halves of one movement ---------------------------------------------
    SoundDesc::sfx(
        "blink_depart",
        ABILITIES,
        Recipe::new(0.3)
            .noise(0.8, 5_000.0, 700.0)
            .tone(0.5, 900.0, 180.0)
            .envelope(0.002, 0.0, 3.0),
        PRIORITY_IMPACT,
    )
    .gain(0.7),
    SoundDesc::sfx(
        "blink_arrive",
        ABILITIES,
        Recipe::new(0.35)
            .noise(0.8, 700.0, 4_500.0)
            .tone(0.5, 180.0, 900.0)
            .envelope(0.002, 0.0, 2.4),
        PRIORITY_IMPACT,
    )
    .gain(0.7),
    // ---- The Ultimate: the largest thing in the catalogue, and it never loses a voice ---
    SoundDesc::sfx(
        "cataclysm_cast",
        ABILITIES,
        Recipe::new(1.5)
            .noise(0.9, 700.0, 1_400.0)
            .tone(0.6, 70.0, 130.0)
            .tremolo(4.0, 0.5)
            .looped(),
        PRIORITY_ULTIMATE,
    )
    .gain(0.7)
    .carries(8.0, 90.0),
    SoundDesc::sfx(
        "cataclysm_impact",
        IMPACTS,
        Recipe::new(2.6)
            .noise(1.0, 3_400.0, 80.0)
            .tone(1.0, 150.0, 22.0)
            .crackle(0.5, 70.0)
            .envelope(0.002, 0.06, 1.8),
        PRIORITY_ULTIMATE,
    )
    .gain(1.0)
    .carries(12.0, 120.0),
    // ---- Ignite: a small ignition, and a bed that many targets share -------------------
    SoundDesc::sfx(
        "ignite_apply",
        IMPACTS,
        Recipe::new(0.22)
            .noise(0.8, 2_000.0, 3_600.0)
            .envelope(0.01, 0.0, 2.6),
        PRIORITY_LOOP,
    )
    .variations(2)
    .gain(0.3)
    .limit(3, 0.09),
    SoundDesc::sfx(
        "ignite_loop",
        IMPACTS,
        Recipe::new(2.2)
            .noise(0.8, 1_300.0, 1_100.0)
            .crackle(0.3, 18.0)
            .tremolo(1.8, 0.4)
            .looped(),
        PRIORITY_LOOP,
    )
    .gain(0.22)
    .carries(3.0, 35.0)
    // The wall of burning targets this phase exists to prevent: four beds at once, however
    // many things are on fire. Milestone 14 measures it at 1, 10 and 50 targets.
    .limit(4, 0.0),
    SoundDesc::sfx(
        "cast_fizzle",
        ABILITIES,
        Recipe::new(0.3)
            .noise(0.7, 2_400.0, 300.0)
            .tone(0.3, 300.0, 90.0)
            .envelope(0.004, 0.0, 3.0),
        PRIORITY_CAST,
    )
    .gain(0.4),
    // ---- Damage feedback: the most frequent sounds in the game, so the tightest limits --
    SoundDesc::sfx(
        "hit_normal",
        IMPACTS,
        Recipe::new(0.16)
            .noise(0.9, 3_600.0, 900.0)
            .tone(0.5, 300.0, 120.0)
            .envelope(0.001, 0.0, 3.2),
        PRIORITY_IMPACT,
    )
    .variations(3)
    .gain(0.35)
    .jitter(0.08, 0.15)
    // Fifty targets take damage on the same tick; five hits is what reads as "a lot of
    // things were hit" without becoming one flat wall of noise.
    .limit(5, 0.035),
    SoundDesc::sfx(
        "hit_critical",
        IMPACTS,
        Recipe::new(0.3)
            .noise(1.0, 5_200.0, 700.0)
            .tone(0.7, 620.0, 150.0)
            .crackle(0.3, 120.0)
            .envelope(0.001, 0.01, 2.6),
        PRIORITY_CRITICAL,
    )
    .variations(3)
    .gain(0.7)
    .jitter(0.05, 0.1)
    .limit(4, 0.04),
    SoundDesc::sfx(
        "heavy_impact",
        IMPACTS,
        Recipe::new(0.6)
            .noise(0.9, 1_600.0, 200.0)
            .tone(0.9, 170.0, 45.0)
            .envelope(0.002, 0.02, 2.2),
        PRIORITY_IMPACT,
    )
    .variations(2)
    .gain(0.8)
    .limit(3, 0.08),
    SoundDesc::sfx(
        "death",
        IMPACTS,
        Recipe::new(0.9)
            .noise(0.9, 1_800.0, 180.0)
            .tone(0.6, 240.0, 60.0)
            .crackle(0.25, 30.0)
            .envelope(0.004, 0.05, 2.0),
        PRIORITY_DEATH,
    )
    .variations(2)
    .gain(0.7)
    .limit(4, 0.05),
    SoundDesc::sfx(
        "death_skeleton",
        IMPACTS,
        Recipe::new(0.9)
            .noise(0.9, 1_800.0, 180.0)
            .tone(0.3, 900.0, 300.0)
            .crackle(0.25, 30.0)
            .envelope(0.004, 0.05, 2.0),
        PRIORITY_DEATH,
    )
    .variations(2)
    .gain(0.7)
    .limit(4, 0.05),
    SoundDesc::sfx(
        "death_acolyte",
        IMPACTS,
        Recipe::new(0.9)
            .noise(0.9, 1_800.0, 180.0)
            .tone(0.5, 420.0, 140.0)
            .crackle(0.25, 30.0)
            .envelope(0.004, 0.05, 2.0),
        PRIORITY_DEATH,
    )
    .variations(2)
    .gain(0.7)
    .limit(4, 0.05),
    SoundDesc::sfx(
        "death_brute",
        IMPACTS,
        Recipe::new(0.9)
            .noise(0.9, 1_800.0, 180.0)
            .tone(0.8, 140.0, 40.0)
            .crackle(0.25, 30.0)
            .envelope(0.004, 0.05, 2.0),
        PRIORITY_DEATH,
    )
    .variations(2)
    .gain(0.7)
    .limit(4, 0.05),
    SoundDesc::sfx(
        "death_ghoul",
        IMPACTS,
        Recipe::new(0.9)
            .noise(0.9, 1_800.0, 180.0)
            .tone(0.5, 700.0, 200.0)
            .crackle(0.25, 30.0)
            .envelope(0.004, 0.05, 2.0),
        PRIORITY_DEATH,
    )
    .variations(2)
    .gain(0.7)
    .limit(4, 0.05),
    SoundDesc::sfx(
        "death_boss",
        IMPACTS,
        Recipe::new(0.9)
            .noise(0.9, 1_800.0, 180.0)
            .tone(0.9, 160.0, 35.0)
            .crackle(0.25, 30.0)
            .envelope(0.004, 0.05, 2.0),
        PRIORITY_DEATH,
    )
    .gain(0.7)
    .limit(4, 0.05),
    // ---- Enemies: darker and lower than the mage, so a gather says whose it is ---------
    SoundDesc::sfx(
        "enemy_cast_quick",
        ABILITIES,
        Recipe::new(0.5)
            .noise(0.7, 700.0, 1_100.0)
            .tremolo(12.0, 0.3)
            .looped(),
        PRIORITY_CAST,
    )
    .gain(0.4)
    .limit(4, 0.0),
    SoundDesc::sfx(
        "enemy_cast_sweep",
        ABILITIES,
        Recipe::new(0.7)
            .noise(0.8, 500.0, 900.0)
            .tone(0.2, 110.0, 130.0)
            .tremolo(7.0, 0.3)
            .looped(),
        PRIORITY_CAST,
    )
    .gain(0.45)
    .limit(4, 0.0),
    SoundDesc::sfx(
        "enemy_cast_heavy",
        ABILITIES,
        Recipe::new(1.0)
            .noise(0.6, 300.0, 600.0)
            .tone(0.4, 70.0, 90.0)
            .tremolo(4.0, 0.35)
            .looped(),
        PRIORITY_CAST,
    )
    .gain(0.5)
    .limit(4, 0.0),
    SoundDesc::sfx(
        "enemy_telegraph",
        ABILITIES,
        Recipe::new(1.0)
            .noise(0.4, 400.0, 400.0)
            .tone(0.5, 180.0, 180.0)
            .tremolo(6.0, 0.4)
            .looped(),
        PRIORITY_CAST,
    )
    .gain(0.4)
    .limit(6, 0.0),
    SoundDesc::sfx(
        "enemy_telegraph_burst",
        IMPACTS,
        Recipe::new(0.9)
            .noise(1.0, 2_000.0, 200.0)
            .tone(0.6, 120.0, 45.0)
            .envelope(0.003, 0.05, 2.2),
        PRIORITY_IMPACT,
    )
    .variations(2)
    .gain(0.75)
    .limit(4, 0.05),
    // ---- Dungeon modifiers: low, unwholesome, and a long way from fire ---------------------
    SoundDesc::sfx(
        "modifier_beams_start",
        ABILITIES,
        Recipe::new(1.6)
            .noise(0.25, 260.0, 520.0)
            .tone(0.7, 62.0, 96.0)
            .tremolo(5.0, 0.4)
            .envelope(0.35, 0.3, 1.6),
        PRIORITY_CAST,
    )
    .gain(0.55)
    .limit(3, 0.2)
    .carries(6.0, 70.0),
    SoundDesc::sfx(
        "modifier_plague_arm",
        ABILITIES,
        Recipe::new(0.35)
            .noise(0.5, 900.0, 300.0)
            .tone(0.35, 340.0, 520.0)
            .crackle(0.7, 60.0)
            .envelope(0.004, 0.0, 2.2),
        PRIORITY_CAST,
    )
    .variations(3)
    .gain(0.3)
    .limit(4, 0.08),
    SoundDesc::sfx(
        "modifier_plague_explode",
        IMPACTS,
        Recipe::new(0.8)
            .noise(1.0, 1_300.0, 140.0)
            .tone(0.5, 140.0, 55.0)
            .crackle(0.5, 140.0)
            .envelope(0.003, 0.04, 2.0),
        PRIORITY_IMPACT,
    )
    .variations(3)
    .gain(0.65)
    .limit(4, 0.06),
    // ---- The key reroll: a dry tick while the dice roll, then one of two stingers ---------
    SoundDesc::sfx(
        "modifier_reroll",
        UI,
        Recipe::new(0.07)
            .tone(0.8, 760.0, 560.0)
            .noise(0.3, 5_200.0, 2_400.0)
            .envelope(0.001, 0.0, 3.2),
        PRIORITY_UI,
    )
    .on(AudioCategory::Ui)
    .gain(0.32)
    .variations(2)
    .jitter(0.06, 0.0)
    .limit(2, 0.04),
    SoundDesc::sfx(
        "modifier_result",
        UI,
        Recipe::new(0.7)
            .tone(0.9, 523.0, 1_046.0)
            .tone(0.4, 784.0, 1_568.0)
            .noise(0.15, 7_000.0, 5_000.0)
            .envelope(0.005, 0.25, 1.8),
        PRIORITY_UI,
    )
    .on(AudioCategory::Ui)
    .gain(0.5)
    .jitter(0.0, 0.0)
    .limit(2, 0.1),
    SoundDesc::sfx(
        "modifier_result_weak",
        UI,
        Recipe::new(0.7)
            .tone(0.9, 523.0, 196.0)
            .tone(0.4, 392.0, 147.0)
            .noise(0.2, 1_800.0, 400.0)
            .envelope(0.005, 0.25, 1.8),
        PRIORITY_UI,
    )
    .on(AudioCategory::Ui)
    .gain(0.5)
    .jitter(0.0, 0.0)
    .limit(2, 0.1),
    // ---- The character ------------------------------------------------------------------
    SoundDesc::sfx(
        "footstep",
        CHARACTERS,
        Recipe::new(0.14)
            .noise(0.8, 2_600.0, 700.0)
            .crackle(0.4, 200.0)
            .envelope(0.001, 0.0, 3.4),
        PRIORITY_FOOTSTEP,
    )
    .variations(4)
    .gain(0.22)
    .jitter(0.12, 0.2)
    .carries(1.5, 25.0)
    .limit(3, 0.12),
    // ---- The arena ----------------------------------------------------------------------
    SoundDesc::sfx(
        "ambient_wind",
        ENVIRONMENT,
        Recipe::new(3.0)
            .noise(1.0, 600.0, 500.0)
            .tremolo(0.35, 0.6)
            .looped(),
        PRIORITY_AMBIENT,
    )
    .on(AudioCategory::Ambient)
    .gain(0.35)
    .limit(1, 0.0),
    SoundDesc::sfx(
        "ambient_ember",
        ENVIRONMENT,
        Recipe::new(0.7)
            .noise(0.6, 2_200.0, 800.0)
            .crackle(0.5, 26.0)
            .envelope(0.05, 0.1, 2.0),
        PRIORITY_AMBIENT,
    )
    .on(AudioCategory::Ambient)
    .variations(3)
    .gain(0.25)
    .carries(6.0, 45.0)
    .limit(2, 0.5),
    // ---- The interface. Never positional; see AudioSystem::play_ui ----------------------
    SoundDesc::sfx(
        "ui_click",
        UI,
        Recipe::new(0.09)
            .tone(1.0, 1_400.0, 900.0)
            .noise(0.2, 6_000.0, 3_000.0)
            .envelope(0.001, 0.0, 3.0),
        PRIORITY_UI,
    )
    .on(AudioCategory::Ui)
    .gain(0.35)
    .jitter(0.0, 0.0)
    .limit(3, 0.03),
    SoundDesc::sfx(
        "ui_select",
        UI,
        Recipe::new(0.14)
            .tone(1.0, 900.0, 1_800.0)
            .noise(0.15, 5_000.0, 4_000.0)
            .envelope(0.002, 0.02, 2.6),
        PRIORITY_UI,
    )
    .on(AudioCategory::Ui)
    .gain(0.4)
    .jitter(0.0, 0.0)
    .limit(3, 0.03),
    SoundDesc::sfx(
        "ui_cancel",
        UI,
        Recipe::new(0.14)
            .tone(1.0, 1_100.0, 520.0)
            .noise(0.15, 4_000.0, 2_000.0)
            .envelope(0.002, 0.02, 2.6),
        PRIORITY_UI,
    )
    .on(AudioCategory::Ui)
    .gain(0.4)
    .jitter(0.0, 0.0)
    .limit(3, 0.03),
];

/// One sound's samples: mono PCM and the rate it was authored at.
///
/// Mono, always. A positional sound has to be placed by the listener, and a stereo source
/// has already decided where it is. Stereo files are folded to mono on load for the same
/// reason, and the fold is announced in the log.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundAsset {
    samples: Vec<f32>,
    sample_rate: u32,
}

impl SoundAsset {
    /// An asset holding `samples` authored at `sample_rate`.
    ///
    /// A zero rate is replaced with 48 kHz and an empty sample list with one of silence:
    /// both are things a malformed file can produce and neither may reach a voice.
    #[must_use]
    pub fn new(samples: Vec<f32>, sample_rate: u32) -> Self {
        let mut samples = samples;
        if samples.is_empty() {
            samples.push(0.0);
        }
        Self {
            samples,
            sample_rate: if sample_rate == 0 {
                48_000
            } else {
                sample_rate
            },
        }
    }

    /// The samples.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Never true — an asset always holds at least one sample. Present because clippy asks
    /// for it beside `len`, and because a caller reading it should get an honest answer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The rate it was authored at.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// How long it plays for, in seconds, at its authored rate.
    #[must_use]
    pub fn duration(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate as f32
    }

    /// How much memory it holds.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.samples.len() * size_of::<f32>()
    }

    /// The sample at `position`, linearly interpolated, or zero past the end.
    ///
    /// Interpolated rather than nearest because pitch variation moves the read head off the
    /// grid on every voice, and nearest-neighbour resampling of noise is audible as a rasp.
    #[must_use]
    pub fn sample_at(&self, position: f64) -> f32 {
        if position < 0.0 {
            return 0.0;
        }
        let index = position as usize;
        let Some(&current) = self.samples.get(index) else {
            return 0.0;
        };
        let next = self.samples.get(index + 1).copied().unwrap_or(0.0);
        let fraction = (position - index as f64) as f32;
        current + (next - current) * fraction
    }
}

/// The samples for every sound, one row per [`SoundId`] and one entry per variation.
#[derive(Debug, Default)]
pub struct SoundBank {
    rows: Vec<Vec<Arc<SoundAsset>>>,
    sample_rate: u32,
    loaded_from_disk: usize,
    bytes: usize,
}

impl SoundBank {
    /// A bank with no samples at all.
    ///
    /// What the game runs with if loading is skipped: every lookup misses, every voice is
    /// silent, and nothing else changes. It is also what the "a sound asset failed to load"
    /// tests use, because that is exactly the state a failure leaves.
    #[must_use]
    pub fn empty(sample_rate: u32) -> Self {
        Self {
            rows: vec![Vec::new(); SOUND_COUNT],
            sample_rate: if sample_rate == 0 {
                48_000
            } else {
                sample_rate
            },
            loaded_from_disk: 0,
            bytes: 0,
        }
    }

    /// Synthesises every sound in the catalogue at `sample_rate`.
    ///
    /// This is the whole asset load: a few million samples of arithmetic, no I/O, no
    /// decoding. Each variation is seeded from the sound's index and the variation's, so the
    /// bank is identical on every machine and across every run.
    #[must_use]
    pub fn synthesised(sample_rate: u32) -> Self {
        let mut bank = Self::empty(sample_rate);
        for id in SoundId::ALL {
            let desc = id.desc();
            let variations = desc.variations.max(1);
            let mut row = Vec::with_capacity(variations as usize);
            for variation in 0..variations {
                let seed = seed_for(id, variation);
                let samples = synth::render(&desc.recipe, bank.sample_rate, seed);
                let asset = SoundAsset::new(samples, bank.sample_rate);
                bank.bytes += asset.bytes();
                row.push(Arc::new(asset));
            }
            bank.rows[id.index()] = row;
        }
        bank
    }

    /// Replaces synthesised variations with WAV files found under `root`.
    ///
    /// `root` is the `assets/audio` directory. A file that is missing changes nothing; a
    /// file that is malformed is skipped with a warning and the synthesised sound stays. The
    /// game is never left without audio because somebody dropped in a bad file.
    pub fn load_overrides(&mut self, root: &Path) {
        for id in SoundId::ALL {
            let desc = id.desc();
            let folder = root.join(desc.folder);
            let variations = desc.variations.max(1);
            let mut replacements: Vec<Arc<SoundAsset>> = Vec::new();

            // `name.wav` replaces a single-variation sound; `name_1.wav` … replace the
            // variations one for one. Both spellings are accepted for every sound, so a
            // one-file override of a three-variation sound is also a legal thing to do.
            if let Some(asset) = self.read(&folder.join(format!("{}.wav", desc.name))) {
                replacements.push(asset);
            }
            for variation in 1..=variations {
                let path = folder.join(format!("{}_{variation}.wav", desc.name));
                if let Some(asset) = self.read(&path) {
                    replacements.push(asset);
                }
            }

            if !replacements.is_empty() {
                self.loaded_from_disk += replacements.len();
                self.bytes += replacements
                    .iter()
                    .map(|asset| asset.bytes())
                    .sum::<usize>();
                self.rows[id.index()] = replacements;
            }
        }
        if self.loaded_from_disk > 0 {
            log::info!(
                "audio: {} variation(s) loaded from {}",
                self.loaded_from_disk,
                root.display()
            );
        }
    }

    /// Reads one WAV file, or `None` if it is missing or unreadable.
    fn read(&self, path: &Path) -> Option<Arc<SoundAsset>> {
        if !path.is_file() {
            return None;
        }
        match wav::read(path) {
            Ok(asset) => Some(Arc::new(asset)),
            Err(error) => {
                log::warn!(
                    "audio: {} could not be read ({error}); the synthesised sound is kept",
                    path.display()
                );
                None
            }
        }
    }

    /// Adds or replaces one sound's variations. What a test uses to build a bank by hand.
    pub fn insert(&mut self, id: SoundId, variations: Vec<Arc<SoundAsset>>) {
        self.bytes += variations.iter().map(|asset| asset.bytes()).sum::<usize>();
        self.rows[id.index()] = variations;
    }

    /// One variation of a sound, or `None` if that sound has no samples.
    ///
    /// The index wraps, so a caller that rolled a variation for a three-variation sound and
    /// then found a one-variation override still gets audio rather than silence.
    #[must_use]
    pub fn get(&self, id: SoundId, variation: usize) -> Option<&Arc<SoundAsset>> {
        let row = self.rows.get(id.index())?;
        if row.is_empty() {
            return None;
        }
        row.get(variation % row.len())
    }

    /// How many variations a sound has. Zero means it will play silently.
    #[must_use]
    pub fn variations(&self, id: SoundId) -> usize {
        self.rows.get(id.index()).map_or(0, Vec::len)
    }

    /// The rate every synthesised sound in this bank was authored at.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// How many sounds have no samples at all.
    #[must_use]
    pub fn missing(&self) -> usize {
        SoundId::ALL
            .into_iter()
            .filter(|id| self.variations(*id) == 0)
            .count()
    }

    /// How much memory the samples occupy.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// How many variations came from disk rather than the synthesiser.
    #[must_use]
    pub fn loaded_from_disk(&self) -> usize {
        self.loaded_from_disk
    }
}

/// The seed one variation of one sound is rendered from.
///
/// A function of the two indices only: the bank does not depend on load order, on the clock
/// or on anything a machine could differ in.
fn seed_for(id: SoundId, variation: u8) -> u64 {
    0x9E37_79B9_7F4A_7C15
        ^ (id.index() as u64).wrapping_mul(0x0000_0100_0000_01B3)
        ^ (variation as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_in_variant_order() {
        // `desc` indexes by the discriminant, so a row in the wrong place would silently
        // give one sound another's category, priority and limits.
        for id in SoundId::ALL {
            assert_eq!(SOUNDS[id.index()].name, id.name(), "{id:?}");
        }
        assert_eq!(SoundId::ALL.len(), SOUND_COUNT);
    }

    #[test]
    fn every_sound_has_a_unique_name_and_sane_numbers() {
        let mut names: Vec<&str> = SOUNDS.iter().map(|desc| desc.name).collect();
        names.sort_unstable();
        let unique = {
            let mut unique = names.clone();
            unique.dedup();
            unique
        };
        assert_eq!(names, unique, "two sounds share a name");

        for desc in &SOUNDS {
            assert!(desc.gain > 0.0 && desc.gain <= 1.5, "{}", desc.name);
            assert!(desc.variations >= 1, "{}", desc.name);
            assert!(
                desc.ref_distance > 0.0 && desc.ref_distance < desc.max_distance,
                "{}",
                desc.name
            );
            assert!(desc.recipe.duration > 0.0, "{}", desc.name);
        }
    }

    #[test]
    fn the_interface_is_never_a_world_sound_and_the_bed_is_never_sfx() {
        assert_eq!(SoundId::UiClick.desc().category, AudioCategory::Ui);
        assert_eq!(SoundId::AmbientWind.desc().category, AudioCategory::Ambient);
        assert_eq!(SoundId::HitNormal.desc().category, AudioCategory::Sfx);
    }

    #[test]
    fn the_sounds_that_must_loop_do_and_the_ones_that_must_not_do_not() {
        for id in [
            SoundId::IgniteLoop,
            SoundId::FirestormLoop,
            SoundId::FlameShieldLoop,
            SoundId::AmbientWind,
            SoundId::FireboltTravel,
            SoundId::MeteorIncoming,
        ] {
            assert!(id.is_looping(), "{id:?} must loop");
        }
        for id in [
            SoundId::HitNormal,
            SoundId::HitCritical,
            SoundId::Death,
            SoundId::MeteorImpact,
            SoundId::UiClick,
        ] {
            assert!(!id.is_looping(), "{id:?} must not loop");
        }
    }

    #[test]
    fn a_synthesised_bank_has_every_variation_the_table_asked_for() {
        // 8 kHz: the same table, a tenth of the samples, so the test is fast.
        let bank = SoundBank::synthesised(8_000);
        assert_eq!(bank.missing(), 0);
        for id in SoundId::ALL {
            let desc = id.desc();
            assert_eq!(bank.variations(id), desc.variations as usize, "{id:?}");
            for variation in 0..bank.variations(id) {
                let asset = bank.get(id, variation).expect("a variation exists");
                assert!(asset.duration() > 0.0, "{id:?}");
                assert!(asset.samples().iter().all(|sample| sample.is_finite()));
            }
        }
        assert!(bank.bytes() > 0);
    }

    #[test]
    fn variations_of_one_sound_are_actually_different() {
        let bank = SoundBank::synthesised(8_000);
        let first = bank.get(SoundId::HitNormal, 0).expect("variation 0");
        let second = bank.get(SoundId::HitNormal, 1).expect("variation 1");
        assert_ne!(first.samples(), second.samples());
    }

    #[test]
    fn an_empty_bank_misses_everything_without_panicking() {
        let bank = SoundBank::empty(48_000);
        assert_eq!(bank.missing(), SOUND_COUNT);
        for id in SoundId::ALL {
            assert!(bank.get(id, 0).is_none(), "{id:?}");
            assert_eq!(bank.variations(id), 0);
        }
    }

    #[test]
    fn a_variation_index_past_the_end_wraps_rather_than_going_silent() {
        let bank = SoundBank::synthesised(8_000);
        let wrapped = bank.get(SoundId::HitNormal, 7).expect("wraps");
        let direct = bank.get(SoundId::HitNormal, 1).expect("variation 1");
        assert_eq!(wrapped.samples(), direct.samples());
    }

    #[test]
    fn an_asset_reads_off_the_grid_and_past_its_end_safely() {
        let asset = SoundAsset::new(vec![0.0, 1.0, 0.0], 48_000);
        assert!((asset.sample_at(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(asset.sample_at(-1.0), 0.0);
        assert_eq!(asset.sample_at(99.0), 0.0);
        assert_eq!(
            asset.sample_at(2.5),
            0.0,
            "the last sample fades to nothing"
        );
    }

    #[test]
    fn a_malformed_asset_is_repaired_rather_than_trusted() {
        let asset = SoundAsset::new(Vec::new(), 0);
        assert_eq!(asset.sample_rate(), 48_000);
        assert_eq!(asset.len(), 1);
        assert!(asset.duration() > 0.0);
    }
}
