//! Combat, heard: the layer that decides what an ability sounds like.
//!
//! ```text
//! Simulation ─ events ─▶ AudioDirector ─▶ AudioSystem ─▶ mixed stereo ─▶ the device
//!             (observe)   (decides)       (voices, mixer, listener)
//! ```
//!
//! # What lives here and what does not
//!
//! `engine-audio` knows what an explosion is *made of* and how loud a thing 30 m away is.
//! This module knows that **Detonate** sounds like one, that Meteor whistles on the way
//! down, and that a burn ticking on fifty targets must not be fifty hit sounds. That split
//! is the same one the rest of the project uses, and it is the same one
//! [`crate::vfx::VfxDirector`] draws for pictures — the two directors are deliberately
//! symmetric, so a change to how an ability reads can be made to both in the same shape.
//!
//! Everything below is driven by *observing* [`CombatEvent`]s, exactly as the damage meter,
//! the floating numbers and the effects are. **Nothing in `engine-game` knows this exists**,
//! no gameplay function is called to make a sound, and deleting this file would leave a game
//! that plays identically in silence.
//!
//! # The two ways a sound starts
//!
//! * **An event.** A cast landed, a rock is falling, somebody blinked, something died.
//! * **A state.** Flame Shield is up; a cast is running; the arena has wind in it. These are
//!   synchronised each frame against the simulation, starting when the state begins and
//!   stopping when it ends — the same rule the VFX director follows, and for the same
//!   reason: deriving a state from events alone needs a second copy of the state machine.
//!
//! # Timing is gameplay's and the picture's, never the sound's
//!
//! A bolt's impact plays when the *cosmetic bolt* arrives, using
//! [`crate::vfx::BOLT_SPEED`] and its two bounds — the same constants the VFX director
//! flies the bolt with — so the crack and the flash are the same moment. The damage itself
//! already landed when the event was emitted; nothing here can move it.
//!
//! # A burn is not fifty hits
//!
//! Damage credited to Ignite plays no hit sound at all. It is a tick on every burning target
//! several times a second, the burning bed already says it is happening, and the honest
//! reading of "fifty targets are on fire" is one fire, not fifty impacts.

use engine_audio::{AudioStats, AudioSystem, Listener, SoundCue, SoundId, VoiceHandle};
use engine_core::math::Vec3;
use engine_game::enemy::{EnemyAbility, TelegraphShape};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::FireAbility;
use engine_game::sim::{CastEnd, Simulation};
use engine_render_data::Camera;

use crate::vfx::{BOLT_MAX_FLIGHT, BOLT_MIN_FLIGHT, BOLT_SPEED, CHEST_FRACTION, LANDING_SPEED};

/// Damage at or above this plays the heavy impact instead of the ordinary one.
///
/// Not a fraction of the target's health: the *hit* is what is being described, and a big
/// number landing should sound big whether it lands on a dummy or on a boss.
pub const HEAVY_DAMAGE: f32 = 40.0;

/// How far the player walks between footsteps, in metres.
///
/// Distance rather than a timer, so a character that is walking, running or being slowed
/// steps in time with its own movement without anything here knowing what its speed is.
pub const STRIDE: f32 = 1.9;

/// Below this speed the character is standing still rather than walking, in metres/second.
const WALK_SPEED: f32 = 0.6;

/// How much louder a landing gets at the top of its range.
const LANDING_FORCE: f32 = 1.6;

/// A cosmetic bolt, and the impact waiting at the end of it.
///
/// The audio twin of [`crate::vfx::VfxDirector`]'s bolt: same line, same flight time, so the
/// two arrive together.
#[derive(Debug, Clone, Copy)]
struct Bolt {
    to: Vec3,
    elapsed: f32,
    duration: f32,
    impact: SoundId,
    /// The travelling bed, stopped when it lands.
    travel: Option<VoiceHandle>,
}

/// A loop the director is holding for something in the world.
#[derive(Debug, Clone, Copy)]
struct Held<Key> {
    key: Key,
    handle: VoiceHandle,
}

/// What the director is holding, for a readout.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AudioDirectorStats {
    /// What the runtime holds.
    pub system: AudioStats,
    /// Bolts in flight.
    pub bolts: usize,
    /// Looping sounds the director is keeping alive.
    pub loops: usize,
    /// Cues the director asked for and the runtime refused.
    pub refused: u64,
}

/// Turns combat into sound.
#[derive(Debug, Default)]
pub struct AudioDirector {
    bolts: Vec<Bolt>,
    /// The burn playing on each target, by entity slot and generation.
    ignited: Vec<Option<(u32, VoiceHandle)>>,
    /// The bed playing for each live zone, by the gameplay id the event carried.
    areas: Vec<Held<u32>>,
    /// The whistle of each rock on its way down, by where it will land.
    incoming: Vec<Held<u32>>,
    /// The rising sound under each enemy attack that is coming, by telegraph id.
    telegraphs: Vec<Held<u32>>,
    /// The gathering sound on each enemy that is casting, by entity slot.
    enemy_casts: Vec<Held<u32>>,
    /// The mage's defensive shell, while Flame Shield is up.
    shield: Option<VoiceHandle>,
    /// The Fire Mastery aura, while it is up.
    mastery: Option<VoiceHandle>,
    /// The gathering sound, while a cast is running.
    cast: Option<VoiceHandle>,
    /// Where the player was last frame, and how far it has walked since its last step.
    last_player: Option<Vec3>,
    stride: f32,
    /// The next id handed to a scheduled impact, so its whistle can be found again.
    next_incoming: u32,
    enabled: bool,
    refused: u64,
}

impl AudioDirector {
    /// A director with nothing playing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bolts: Vec::with_capacity(16),
            ignited: Vec::with_capacity(64),
            areas: Vec::with_capacity(8),
            incoming: Vec::with_capacity(8),
            telegraphs: Vec::with_capacity(8),
            enemy_casts: Vec::with_capacity(8),
            enabled: true,
            ..Self::default()
        }
    }

    /// Whether the director makes any sound.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Turns every sound on or off.
    ///
    /// Off stops new cues and ends the ones playing, so "with audio" and "without audio" are
    /// two measurable states of one build rather than two builds — which is what the
    /// performance audit compares.
    pub fn set_enabled(&mut self, enabled: bool, audio: &mut AudioSystem) {
        self.enabled = enabled;
        if !enabled {
            self.clear(audio);
        }
    }

    /// Ends everything the director is holding. For a scene change or a combat reset.
    pub fn clear(&mut self, audio: &mut AudioSystem) {
        audio.stop_all();
        self.bolts.clear();
        self.ignited.clear();
        self.areas.clear();
        self.incoming.clear();
        self.telegraphs.clear();
        self.enemy_casts.clear();
        self.shield = None;
        self.mastery = None;
        self.cast = None;
        self.last_player = None;
        self.stride = 0.0;
    }

    /// What the director is holding.
    #[must_use]
    pub fn stats(&self, audio: &AudioSystem) -> AudioDirectorStats {
        AudioDirectorStats {
            system: audio.stats(),
            bolts: self.bolts.len(),
            loops: self.areas.len()
                + self.telegraphs.len()
                + self.enemy_casts.len()
                + self.incoming.len()
                + self.ignited.iter().filter(|slot| slot.is_some()).count()
                + usize::from(self.shield.is_some())
                + usize::from(self.mastery.is_some())
                + usize::from(self.cast.is_some()),
            refused: self.refused,
        }
    }

    /// Plays an interface sound, which has no place in the world.
    ///
    /// The one entry point that is not driven by an event: a button press is not something
    /// the simulation announces, because pressing a button is not something that happens in
    /// the world.
    pub fn play_ui(&mut self, sound: SoundId, audio: &mut AudioSystem) {
        if !self.enabled {
            return;
        }
        self.request(SoundCue::ui(sound), audio);
    }

    /// Points the ear at wherever the gameplay camera is looking.
    ///
    /// The camera stays authoritative: this reads it, and there is no second camera to
    /// drift. Called once a frame, after the camera has been updated.
    pub fn set_listener(&self, camera: &Camera, audio: &mut AudioSystem) {
        audio.set_listener(Listener::new(
            camera.position(),
            camera.forward(),
            camera.up(),
        ));
    }

    /// Reads a frame's combat events and starts what they call for.
    ///
    /// Takes the simulation because a sound needs to know how tall a target is and where its
    /// chest is — questions about the *world*, not about the event. Nothing here writes to
    /// the simulation, and it is borrowed immutably to say so.
    pub fn observe(&mut self, simulation: &Simulation, audio: &mut AudioSystem) {
        if !self.enabled {
            return;
        }
        for index in 0..simulation.events().len() {
            let event = simulation.events()[index];
            self.observe_one(&event, simulation, audio);
        }
    }

    /// Starts whatever one event calls for.
    fn observe_one(
        &mut self,
        event: &CombatEvent,
        simulation: &Simulation,
        audio: &mut AudioSystem,
    ) {
        match *event {
            // The moment an ability goes off. Instant or the end of a cast: either way this
            // is when the spell leaves the caster, so it is when the caster is heard.
            CombatEvent::AbilityUsed {
                source, ability, ..
            } => {
                let Some(fire) = ability.fire() else {
                    return;
                };
                let origin = self.chest_of(source, simulation);
                if let Some(sound) = activation_sound(fire) {
                    self.request(SoundCue::at(sound, origin), audio);
                }
                // Detonate is heard where it goes off, which is the target it consumed the
                // burn from — the same centre the executor collected its hit list around.
                if fire == FireAbility::Detonate {
                    let centre = simulation
                        .player()
                        .target
                        .and_then(|target| simulation.world().position(target))
                        .unwrap_or(origin);
                    self.request(SoundCue::at(SoundId::DetonateExplosion, centre), audio);
                }
            }
            // An enemy attack that is coming. The sound rises where the attack will land,
            // not where the caster is: what the player needs to hear is *where not to be*.
            CombatEvent::TelegraphStarted {
                id, anchor, shape, ..
            } => {
                let sound = match shape {
                    TelegraphShape::Cone { .. } => SoundId::FlameLanceCast,
                    _ => SoundId::MeteorIncoming,
                };
                if let Some(handle) = self.request(SoundCue::at(sound, anchor).looped(true), audio)
                {
                    self.telegraphs.push(Held { key: id, handle });
                }
            }
            CombatEvent::TelegraphEnded {
                id,
                anchor,
                resolved,
                ..
            } => {
                if let Some(index) = self.telegraphs.iter().position(|held| held.key == id) {
                    let held = self.telegraphs.swap_remove(index);
                    audio.stop(held.handle);
                }
                if resolved {
                    self.request(SoundCue::at(SoundId::DetonateExplosion, anchor), audio);
                }
            }
            // A cast started: the gathering bed runs for exactly as long as the cast does,
            // because it is stopped by the event that ends it.
            CombatEvent::CastStarted {
                source, ability, ..
            } => {
                if let Some(enemy) = ability.enemy() {
                    self.begin_enemy_cast(source, enemy, simulation, audio);
                    return;
                }
                let Some(sound) = ability.fire().and_then(charge_sound) else {
                    return;
                };
                if let Some(handle) = self.cast.take() {
                    audio.stop(handle);
                }
                let origin = self.chest_of(source, simulation);
                self.cast = self.request(SoundCue::at(sound, origin), audio);
            }
            // A cast ended. A cast that did not land says so: the gather stops and a short
            // fizzle takes its place, which is the feedback for "that did not happen".
            CombatEvent::CastEnded {
                source,
                ability,
                end,
                ..
            } if ability.is_enemy() => {
                self.end_enemy_cast(source, audio);
                if end != CastEnd::Completed {
                    let origin = self.chest_of(source, simulation);
                    self.request(SoundCue::at(SoundId::CastFizzle, origin), audio);
                }
            }
            CombatEvent::CastEnded { source, end, .. } => {
                if let Some(handle) = self.cast.take() {
                    audio.stop(handle);
                }
                if end != CastEnd::Completed {
                    let origin = self.chest_of(source, simulation);
                    self.request(SoundCue::at(SoundId::CastFizzle, origin), audio);
                }
            }
            // A bolt: the line it flew, with its impact played when the picture arrives.
            CombatEvent::ProjectileLaunched {
                target,
                from,
                to,
                ability,
                ..
            } => {
                let height = simulation.world().height(target);
                let origin = from + Vec3::new(0.0, CHEST_FRACTION * 1.7, 0.0);
                let destination = to + Vec3::new(0.0, height * CHEST_FRACTION, 0.0);
                self.launch_bolt(origin, destination, ability.fire(), audio);
            }
            // The rock is committed to. The whistle plays for the delay gameplay is counting
            // down, at the place it will land, so the player can hear where to not be.
            CombatEvent::ImpactScheduled {
                ability, position, ..
            } => {
                let sound = match ability.fire() {
                    Some(FireAbility::Cataclysm) => SoundId::CataclysmCast,
                    _ => SoundId::MeteorIncoming,
                };
                let key = self.next_incoming;
                self.next_incoming = self.next_incoming.wrapping_add(1);
                if let Some(handle) = self.request(SoundCue::at(sound, position), audio) {
                    self.incoming.push(Held { key, handle });
                }
            }
            // It landed. The whistle nearest the impact is the one that was for it.
            CombatEvent::ImpactLanded {
                ability, position, ..
            } => {
                if let Some(held) = self.incoming.first().copied() {
                    audio.stop(held.handle);
                    self.incoming.remove(0);
                }
                let sound = match ability.fire() {
                    Some(FireAbility::Cataclysm) => SoundId::CataclysmImpact,
                    Some(FireAbility::Pyroblast) => SoundId::PyroblastImpact,
                    _ => SoundId::MeteorImpact,
                };
                self.request(SoundCue::at(sound, position), audio);
            }
            // Fire on the ground: one bed for the zone, stopped when the zone burns out.
            CombatEvent::AreaEffectStarted { id, position, .. } => {
                if let Some(handle) = self.request(
                    SoundCue::at(SoundId::FirestormLoop, position).looped(true),
                    audio,
                ) {
                    self.areas.push(Held { key: id, handle });
                }
            }
            CombatEvent::AreaEffectEnded { id, .. } => {
                if let Some(index) = self.areas.iter().position(|held| held.key == id) {
                    let held = self.areas.swap_remove(index);
                    audio.stop(held.handle);
                }
            }
            // Damage. The one place the limiter matters most: fifty targets take a hit on
            // the same tick and the catalogue's minimum interval turns that into one hit.
            CombatEvent::Damage(damage) => {
                // A burn ticking is not an impact. The bed says the target is on fire.
                if damage.ability.fire() == Some(FireAbility::Ignite) {
                    return;
                }
                let sound = if damage.critical {
                    SoundId::HitCritical
                } else if damage.amount >= HEAVY_DAMAGE {
                    SoundId::HeavyImpact
                } else {
                    SoundId::HitNormal
                };
                let position = damage.position + Vec3::new(0.0, 1.0, 0.0);
                self.request(SoundCue::at(sound, position), audio);
            }
            CombatEvent::Death { entity, .. } => {
                let position = self.chest_of(entity, simulation);
                self.extinguish(entity, audio);
                self.request(SoundCue::at(SoundId::Death, position), audio);
            }
            // A teleport is two sounds, because it is two places.
            CombatEvent::Teleported { from, to, .. } => {
                let lift = Vec3::new(0.0, 1.0, 0.0);
                self.request(SoundCue::at(SoundId::BlinkDepart, from + lift), audio);
                self.request(SoundCue::at(SoundId::BlinkArrive, to + lift), audio);
            }
            // Burning: one bed per target, refreshed rather than stacked, and capped by the
            // catalogue so a pack on fire is a fire rather than a wall.
            CombatEvent::StatusApplied { target, .. } => self.ignite(target, simulation, audio),
            CombatEvent::StatusRemoved { target, .. } => self.extinguish(target, audio),
            CombatEvent::Jumped { position, .. } => {
                self.request(SoundCue::at(SoundId::Jump, position), audio);
            }
            // A hard landing is a different sound, not a louder one: the player should be
            // able to tell a step down from a fall without watching.
            CombatEvent::Landed {
                position, speed, ..
            } => {
                let (sound, gain) = if speed >= LANDING_SPEED {
                    (
                        SoundId::LandHeavy,
                        (speed / LANDING_SPEED).clamp(1.0, LANDING_FORCE),
                    )
                } else {
                    (SoundId::Land, 1.0)
                };
                self.request(SoundCue::at(sound, position).with_gain(gain), audio);
            }
            // Nothing to hear, or a state the frame synchronises instead. A phase change is
            // seen rather than heard: this phase authors no new cues.
            CombatEvent::Healing { .. } | CombatEvent::PhaseChanged { .. } => {}
        }
    }

    /// Advances bolts, follows anchors, and synchronises the sounds that follow a state.
    ///
    /// `delta` is real time, not simulation time: a sound is presentation, and a bolt in the
    /// air has to arrive when the picture of it does.
    pub fn update(&mut self, simulation: &Simulation, delta: f32, audio: &mut AudioSystem) {
        if !self.enabled {
            return;
        }
        self.advance_bolts(delta, audio);
        self.sync_states(simulation, audio);
        self.walk(simulation, delta, audio);
        self.prune(audio);

        // Where an anchored sound's owner is now. An entity that has gone answers `None`,
        // and the sound riding it stops rather than burning over an empty patch of floor.
        let game = simulation.world();
        audio.update(delta, |key| {
            let id = game.id_at(key as usize)?;
            if !game.is_alive(id) {
                return None;
            }
            game.position(id)
                .map(|position| position + Vec3::new(0.0, game.height(id) * CHEST_FRACTION, 0.0))
        });
    }

    /// Starts the gathering sound on an enemy that has begun a cast.
    fn begin_enemy_cast(
        &mut self,
        source: EntityId,
        ability: EnemyAbility,
        simulation: &Simulation,
        audio: &mut AudioSystem,
    ) {
        self.end_enemy_cast(source, audio);
        // Reused from the player's own casts rather than given sounds of their own: a
        // gather is a gather, and this phase adds no new audio assets.
        let sound = match ability {
            // The dungeon's monsters and its boss are deliberately absent from this table's
            // right-hand side: the phase adds no ability sounds, and every new attack reuses
            // one of the three gathers that already exist.
            EnemyAbility::GroundBlast
            | EnemyAbility::RitualNova
            | EnemyAbility::Ashfall
            | EnemyAbility::ImmolationNova => SoundId::PyroblastCast,
            EnemyAbility::Cleave
            | EnemyAbility::WardensSweep
            | EnemyAbility::AshBurst
            | EnemyAbility::CinderPool
            | EnemyAbility::CinderRain => SoundId::FlameLanceCast,
            EnemyAbility::Bolt
            | EnemyAbility::Swing
            | EnemyAbility::Sunder
            | EnemyAbility::Mend
            | EnemyAbility::EmberLance => SoundId::FireboltCast,
        };
        let origin = self.chest_of(source, simulation);
        let key = source.index() as u32;
        if let Some(handle) = self.request(
            SoundCue::at(sound, origin).anchored_to(key).looped(true),
            audio,
        ) {
            self.enemy_casts.push(Held { key, handle });
        }
    }

    /// Stops it, however the cast ended.
    fn end_enemy_cast(&mut self, source: EntityId, audio: &mut AudioSystem) {
        let key = source.index() as u32;
        if let Some(index) = self.enemy_casts.iter().position(|held| held.key == key) {
            let held = self.enemy_casts.swap_remove(index);
            audio.stop(held.handle);
        }
    }

    /// Starts a bolt, and the impact that will play when it lands.
    fn launch_bolt(
        &mut self,
        from: Vec3,
        to: Vec3,
        ability: Option<FireAbility>,
        audio: &mut AudioSystem,
    ) {
        let distance = (to - from).length();
        // The same three constants the VFX director flies its bolt with. Shared rather than
        // copied: two numbers that must be equal should be one number.
        let duration = (distance / BOLT_SPEED).clamp(BOLT_MIN_FLIGHT, BOLT_MAX_FLIGHT);
        let travel = self.request(SoundCue::at(SoundId::FireboltTravel, from), audio);
        self.bolts.push(Bolt {
            to,
            elapsed: 0.0,
            duration,
            impact: ability.map_or(SoundId::FireboltImpact, impact_sound),
            travel,
        });
    }

    /// Moves every bolt along its line, playing its impact when it arrives.
    fn advance_bolts(&mut self, delta: f32, audio: &mut AudioSystem) {
        let mut index = 0;
        while index < self.bolts.len() {
            let bolt = &mut self.bolts[index];
            bolt.elapsed += delta;
            if bolt.elapsed < bolt.duration {
                index += 1;
                continue;
            }
            let bolt = self.bolts.swap_remove(index);
            if let Some(handle) = bolt.travel {
                audio.stop(handle);
            }
            self.request(SoundCue::at(bolt.impact, bolt.to), audio);
        }
    }

    /// Starts and stops the sounds that follow a *state* rather than a moment.
    fn sync_states(&mut self, simulation: &Simulation, audio: &mut AudioSystem) {
        let player = simulation.player().entity;
        let slot = player.index() as u32;

        let shielded = simulation.mage().flame_shield.is_active();
        Self::sync_loop(
            &mut self.shield,
            shielded,
            SoundCue::at(SoundId::FlameShieldLoop, simulation.player_position()).anchored_to(slot),
            audio,
            &mut self.refused,
        );

        let mastered = simulation.mage().mastery_buff.is_active();
        Self::sync_loop(
            &mut self.mastery,
            mastered,
            SoundCue::at(SoundId::FireMasteryLoop, simulation.player_position()).anchored_to(slot),
            audio,
            &mut self.refused,
        );
    }

    /// Starts `cue` when `wanted` becomes true and stops it when it becomes false.
    fn sync_loop(
        held: &mut Option<VoiceHandle>,
        wanted: bool,
        cue: SoundCue,
        audio: &mut AudioSystem,
        refused: &mut u64,
    ) {
        match (wanted, *held) {
            (true, Some(handle)) if audio.is_playing(handle) => {}
            (true, _) => {
                *held = audio.play(cue.looped(true));
                if held.is_none() {
                    *refused += 1;
                }
            }
            (false, Some(handle)) => {
                audio.stop(handle);
                *held = None;
            }
            (false, None) => {}
        }
    }

    /// Footsteps, from how far the character has actually walked.
    ///
    /// Driven by movement rather than by a timer: a character standing still takes no steps
    /// however long it stands, and one that is slowed steps more slowly without this knowing
    /// what a slow is. The ground check comes from the collision result the simulation
    /// already produced, so a character in the air is silent without a second guess at
    /// whether it is jumping.
    fn walk(&mut self, simulation: &Simulation, delta: f32, audio: &mut AudioSystem) {
        let position = simulation.player_position();
        let Some(previous) = self.last_player.replace(position) else {
            return;
        };
        if delta <= 0.0 {
            return;
        }
        let moved = position - previous;
        let travelled = Vec3::new(moved.x, 0.0, moved.z).length();
        if !simulation.last_move().grounded || travelled / delta < WALK_SPEED {
            // A character that stops mid-stride starts the next one fresh rather than
            // stepping the instant it moves again.
            self.stride = 0.0;
            return;
        }
        self.stride += travelled;
        if self.stride >= STRIDE {
            self.stride -= STRIDE;
            self.request(SoundCue::at(SoundId::Footstep, position), audio);
        }
    }

    /// Puts a burn on a target, or refreshes the one already there.
    fn ignite(&mut self, target: EntityId, simulation: &Simulation, audio: &mut AudioSystem) {
        let position = self.chest_of(target, simulation);
        self.request(SoundCue::at(SoundId::IgniteApply, position), audio);

        let slot = target.index();
        if self.ignited.len() <= slot {
            self.ignited.resize(slot + 1, None);
        }
        // Already burning: the bed keeps playing rather than being restarted, which would
        // put a click on every refresh — and a refresh happens on every Firebolt.
        if let Some((generation, handle)) = self.ignited[slot]
            && generation == target.generation()
            && audio.is_playing(handle)
        {
            return;
        }
        let handle = self.request(
            SoundCue::at(SoundId::IgniteLoop, position)
                .anchored_to(slot as u32)
                .looped(true),
            audio,
        );
        self.ignited[slot] = handle.map(|handle| (target.generation(), handle));
    }

    /// Takes the burn off a target, if it has one.
    fn extinguish(&mut self, target: EntityId, audio: &mut AudioSystem) {
        let slot = target.index();
        let Some(Some((generation, handle))) = self.ignited.get(slot).copied() else {
            return;
        };
        if generation != target.generation() {
            return;
        }
        audio.stop(handle);
        self.ignited[slot] = None;
    }

    /// Forgets the loops that ended without the director being told.
    fn prune(&mut self, audio: &AudioSystem) {
        for entry in &mut self.ignited {
            if let Some((_, handle)) = *entry
                && !audio.is_playing(handle)
            {
                *entry = None;
            }
        }
        self.areas.retain(|held| audio.is_playing(held.handle));
        self.incoming.retain(|held| audio.is_playing(held.handle));
        self.telegraphs.retain(|held| audio.is_playing(held.handle));
        self.enemy_casts
            .retain(|held| audio.is_playing(held.handle));
        for held in [&mut self.shield, &mut self.mastery, &mut self.cast] {
            if let Some(handle) = *held
                && !audio.is_playing(handle)
            {
                *held = None;
            }
        }
    }

    /// Where an entity's chest is, or its feet if it has gone.
    fn chest_of(&self, entity: EntityId, simulation: &Simulation) -> Vec3 {
        let world = simulation.world();
        let position = world.position(entity).unwrap_or(Vec3::ZERO);
        position + Vec3::new(0.0, world.height(entity).max(1.0) * CHEST_FRACTION, 0.0)
    }

    /// Plays a cue, counting a refusal.
    fn request(&mut self, cue: SoundCue, audio: &mut AudioSystem) -> Option<VoiceHandle> {
        let handle = audio.play(cue);
        if handle.is_none() {
            self.refused += 1;
        }
        handle
    }
}

/// What an ability sounds like at the moment it goes off, if it makes a sound of its own.
///
/// `None` for the abilities whose *whole* sound is somewhere else: Flame Lance and Pyroblast
/// are their cast beds and then their impacts, and Blink is its two ends.
#[must_use]
pub fn activation_sound(ability: FireAbility) -> Option<SoundId> {
    match ability {
        FireAbility::Firebolt => Some(SoundId::FireboltCast),
        FireAbility::FlameCone => Some(SoundId::FlameConeCast),
        FireAbility::Meteor => Some(SoundId::MeteorCast),
        FireAbility::Firestorm => Some(SoundId::FirestormCast),
        FireAbility::Detonate => Some(SoundId::DetonateCast),
        FireAbility::FireMastery => Some(SoundId::FireMasteryActivate),
        FireAbility::FlameShield => Some(SoundId::FlameShieldActivate),
        FireAbility::FlameLance | FireAbility::Pyroblast | FireAbility::Cataclysm => None,
        FireAbility::Blink | FireAbility::Ignite => None,
    }
}

/// The bed that runs while an ability is being cast, if it has a cast at all.
#[must_use]
pub fn charge_sound(ability: FireAbility) -> Option<SoundId> {
    match ability {
        FireAbility::FlameLance => Some(SoundId::FlameLanceCast),
        FireAbility::Pyroblast => Some(SoundId::PyroblastCast),
        FireAbility::Cataclysm => Some(SoundId::CataclysmCast),
        // Everything else is instant, or its cast is short enough that the activation is the
        // whole sound. A bed on a 1.5 s Firebolt would be heard as a stutter, not a gather.
        _ => None,
    }
}

/// What an ability sounds like when it lands on something.
#[must_use]
pub fn impact_sound(ability: FireAbility) -> SoundId {
    match ability {
        FireAbility::FlameLance => SoundId::FlameLanceImpact,
        FireAbility::Pyroblast => SoundId::PyroblastImpact,
        FireAbility::Meteor => SoundId::MeteorImpact,
        FireAbility::Detonate => SoundId::DetonateExplosion,
        FireAbility::Cataclysm => SoundId::CataclysmImpact,
        _ => SoundId::FireboltImpact,
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_audio::{DEFAULT_MAX_VOICES, SoundBank};

    fn runtime() -> AudioSystem {
        AudioSystem::with_bank(SoundBank::synthesised(8_000), 8_000, DEFAULT_MAX_VOICES)
    }

    #[test]
    fn every_ability_on_the_bar_makes_a_sound_somewhere() {
        // The acceptance criterion, as a test: an ability with no activation sound has to
        // have a cast bed, and every ability has an impact.
        for ability in FireAbility::BAR {
            let heard = activation_sound(ability).is_some()
                || charge_sound(ability).is_some()
                || matches!(ability, FireAbility::Blink);
            assert!(heard, "{ability:?} makes no sound when it is used");
        }
    }

    #[test]
    fn the_interface_can_be_heard_without_the_world() {
        let mut audio = runtime();
        let mut director = AudioDirector::new();
        director.play_ui(SoundId::UiClick, &mut audio);
        assert_eq!(audio.playing(SoundId::UiClick), 1);
    }

    #[test]
    fn a_disabled_director_makes_no_sound_at_all() {
        let mut audio = runtime();
        let mut director = AudioDirector::new();
        director.set_enabled(false, &mut audio);
        director.play_ui(SoundId::UiClick, &mut audio);
        assert!(audio.is_idle());
    }
}
