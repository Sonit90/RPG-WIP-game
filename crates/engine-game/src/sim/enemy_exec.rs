//! The AI's half of a tick: perceive, decide, move, and resolve what was asked for.
//!
//! ```text
//! for each enemy:
//!    perceive  ── threat table + world  ─▶ AiPercept
//!    think     ── EnemyBrain::think     ─▶ AiDecision { intent, request }
//!    intent    ── stored for movement, which runs later in the same tick
//!    request   ── validated and executed here: casts, telegraphs, damage
//! ```
//!
//! # This file is the *combat* half, not the AI half
//!
//! [`crate::enemy::brain`] decides what an enemy wants. Everything that actually changes the
//! world — starting a cast, announcing a telegraph, collecting who is inside a shape,
//! dealing damage, ending a fight — happens here, through the same
//! [`Simulation::deal_damage`] every player ability goes through. An enemy has no privileged
//! path to anybody's health.
//!
//! # Why the intent is stored rather than applied
//!
//! Movement integration runs once, for every body, later in the tick. Writing a position
//! here would mean two systems moved bodies and the character controller was only one of
//! them. Instead the brain's intent is parked on the runtime and `integrate_bodies` reads
//! it, so an enemy walks with exactly the controller the player walks with — acceleration,
//! ground contact, collision and all.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::enemy::brain::aimed_shape;
use crate::enemy::brain::{AiPercept, AiRequest, AiState, AiTarget, CastStop, EnemyBrain};
use crate::enemy::{EnemyAim, EnemyKind, TelegraphShape};
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::movement::MoveIntent;
use crate::sim::CastEnd;
use crate::world::horizontal_distance;

use super::Simulation;

/// One enemy's runtime: its brain, and what the last decision asked the body to do.
#[derive(Debug, Clone, PartialEq)]
pub struct EnemyRuntime {
    /// The state machine.
    pub brain: EnemyBrain,
    /// What movement should do with this body this tick.
    pub intent: MoveIntent,
    /// The telegraph this enemy currently has up, if any.
    pub telegraph: Option<u32>,
}

impl EnemyRuntime {
    /// A runtime for a `kind` spawned at `home`.
    #[must_use]
    pub fn new(kind: EnemyKind, home: Vec3, facing_yaw: f32) -> Self {
        Self {
            brain: EnemyBrain::new(kind, home, facing_yaw),
            intent: MoveIntent::NONE,
            telegraph: None,
        }
    }
}

impl Simulation {
    /// Runs every enemy's brain and carries out what it asked for.
    ///
    /// Called once a tick, before movement, so an intent decided this tick is walked this
    /// tick rather than next.
    pub(super) fn tick_ai(&mut self, step: f32) {
        // Taken out of `self` so the loop can call `&mut self` methods; put back at the end,
        // keeping every allocation. The same pattern `apply_pending_commands` uses.
        let mut enemies = std::mem::take(&mut self.enemies);
        // Indexed rather than iterated: the body needs `&mut self` *and* a second look at
        // this slot after executing, and an iterator over `enemies` would hold a borrow
        // across both.
        #[allow(
            clippy::needless_range_loop,
            reason = "the slot index is used again inside the body"
        )]
        for slot in 0..enemies.len() {
            let Some(runtime) = enemies[slot].as_mut() else {
                continue;
            };
            let Some(entity) = self.world.id_at(slot) else {
                // The slot was freed under it: nothing to drive.
                continue;
            };
            // A corpse is not tracked. Its brain has already been through `die` — which
            // dropped its target, its threat and its cast — and everything downstream of it
            // has been cleaned up by the death itself: its burn, its statuses, its telegraph
            // and the ground it lit. Thinking again would produce `AiDecision::IDLE` at the
            // cost of a full perception pass over the world, which in a cleared dungeon room
            // is several scans a tick for enemies nobody is fighting.
            //
            // Deliberately keyed on the *brain* rather than on the world: an entity killed
            // outside the combat path has a body at zero health and a brain that has not
            // noticed, and that one still has to think once so it can die properly.
            if runtime.brain.state() == AiState::Dead {
                continue;
            }

            let percept = self.perceive(entity, &runtime.brain);
            // The cast as it was *before* thinking: a cast that completes this tick is taken
            // off the brain by `think`, and the anchor and shape it was started with are
            // exactly what has to resolve. Reading them afterwards would find nothing and
            // fall back to the table's neutral shape - a cone pointing wherever the table
            // said, which is not where the telegraph was drawn.
            let running = runtime.brain.cast();
            let decision = runtime.brain.think(&percept, step);
            runtime.intent = decision.intent;

            let telegraph = runtime.telegraph;
            let request = decision.request;
            let brain_state = runtime.brain.state();
            let started = runtime.brain.cast();

            // Put the runtime back before executing: executing needs `&mut self`, and the
            // execution can itself kill things and read other enemies' state.
            if let Some(phase) = decision.phase {
                self.announce_phase(entity, phase, runtime.brain.phase_name());
            }

            match request {
                AiRequest::None => {}
                AiRequest::Instant(ability) => {
                    let victim = recipient_of(ability, &percept);
                    let aim = cast_anchor(ability, entity, victim, self);
                    self.execute_enemy_ability(entity, ability, victim, aim);
                }
                AiRequest::BeginCast(ability) => {
                    if let Some(cast) = started {
                        let id = self.next_telegraph_id();
                        if let Some(runtime) = enemies[slot].as_mut() {
                            runtime.telegraph = Some(id);
                        }
                        self.announce_cast_start(entity, ability, cast.duration);
                        self.announce_telegraph(
                            id,
                            entity,
                            ability,
                            cast.anchor,
                            cast.shape,
                            cast.duration,
                        );
                    }
                }
                AiRequest::CompleteCast(ability) => {
                    let cast = running;
                    let anchor = cast.map_or_else(
                        || self.world.position(entity).unwrap_or(Vec3::ZERO),
                        |cast| cast.anchor,
                    );
                    if let Some(id) = telegraph {
                        self.announce_telegraph_end(id, anchor, true);
                        if let Some(runtime) = enemies[slot].as_mut() {
                            runtime.telegraph = None;
                        }
                    }
                    self.announce_cast_end(entity, ability, CastEnd::Completed);
                    // The shape the telegraph drew, not a fresh one: they are the same
                    // value, and that is the whole guarantee.
                    let shape = cast.map_or(ability.definition().shape, |cast| cast.shape);
                    // Whoever the cast was started against — for a heal that is an ally, and
                    // re-asking the percept here would find the thing the enemy is fighting.
                    let recipient = cast
                        .map(|cast| cast.target)
                        .and_then(|target| self.describe_target(entity, target))
                        .or_else(|| recipient_of(ability, &percept));
                    self.execute_enemy_shape(entity, ability, anchor, shape, recipient);
                }
                AiRequest::CancelCast(ability) => {
                    let cast = running;
                    let anchor = cast.map_or_else(
                        || self.world.position(entity).unwrap_or(Vec3::ZERO),
                        |cast| cast.anchor,
                    );
                    if let Some(id) = telegraph {
                        self.announce_telegraph_end(id, anchor, false);
                        if let Some(runtime) = enemies[slot].as_mut() {
                            runtime.telegraph = None;
                        }
                    }
                    let reason = if brain_state == AiState::Dead {
                        CastEnd::CasterDied
                    } else {
                        CastEnd::Cancelled
                    };
                    self.announce_cast_end(entity, ability, reason);
                }
            }
        }
        self.enemies = enemies;
        self.sync_encounter();
    }

    /// One entity described the way the AI sees it, from `entity`'s point of view.
    ///
    /// The same edge-to-edge distance [`Simulation::perceive`] measures, so a range check
    /// against this and a range check against a percept cannot disagree.
    fn describe_target(&self, entity: EntityId, candidate: EntityId) -> Option<AiTarget> {
        if !self.world.is_alive(candidate) {
            return None;
        }
        let position = self.world.position(entity)?;
        let target_position = self.world.position(candidate)?;
        let gap = horizontal_distance(position, target_position)
            - self.world.radius(candidate)
            - self.world.radius(entity);
        Some(AiTarget {
            entity: candidate,
            position: target_position,
            distance: gap.max(0.0),
        })
    }

    /// The most wounded living ally of `entity` within `range`, itself included.
    ///
    /// Linear over the world, like every other targeting query in this crate. A pack is a
    /// handful of entities and a map keyed by faction would cost more to maintain than the
    /// scan costs to run.
    fn most_wounded_ally(&self, entity: EntityId, range: f32) -> Option<AiTarget> {
        let faction = self.world.faction(entity)?;
        let mut best: Option<(f32, AiTarget)> = None;
        for candidate in self.world.entities() {
            if !self.world.is_alive(candidate) {
                continue;
            }
            if self.world.faction(candidate) != Some(faction) {
                continue;
            }
            let Some(health) = self.world.health(candidate) else {
                continue;
            };
            let fraction = health.fraction();
            if fraction >= 1.0 {
                continue;
            }
            let Some(described) = self.describe_target(entity, candidate) else {
                continue;
            };
            if described.distance > range {
                continue;
            }
            // Ties break towards the lower entity slot, which `<` gives for free: the scan
            // is in slot order, so the same pack in the same state always heals the same
            // target.
            if best.is_none_or(|(best_fraction, _)| fraction < best_fraction) {
                best = Some((fraction, described));
            }
        }
        best.map(|(_, target)| target)
    }

    /// What one enemy can see this tick.
    fn perceive(&mut self, entity: EntityId, brain: &EnemyBrain) -> AiPercept {
        let position = self.world.position(entity).unwrap_or(Vec3::ZERO);
        let alive = self.world.is_alive(entity);
        let health_fraction = self
            .world
            .health(entity)
            .map_or(0.0, crate::combat::Health::fraction);
        // Only asked for by an archetype that can actually heal. The scan is cheap, but it is
        // a scan per enemy per tick, and most of the roster has no use for the answer.
        let heal_range = brain
            .abilities()
            .filter(|ability| ability.definition().effect.is_healing())
            .map(|ability| ability.definition().range)
            .fold(0.0_f32, f32::max);
        let wounded_ally = (heal_range > 0.0)
            .then(|| self.most_wounded_ally(entity, heal_range))
            .flatten();

        // The target is the highest threat that is still a legal victim. "Legal" is asked of
        // the world here rather than inside the threat table, which is what keeps the table
        // free of any dependency on the world and testable with a closure.
        let world = &self.world;
        let faction = world.faction(entity);
        let target = brain.threat().highest(|candidate| {
            world.is_alive(candidate)
                && faction.is_some_and(|mine| {
                    world
                        .faction(candidate)
                        .is_some_and(|theirs| mine.is_hostile_to(theirs))
                })
        });

        let describe = |candidate: EntityId| -> Option<AiTarget> {
            let target_position = self.world.position(candidate)?;
            let gap = horizontal_distance(position, target_position)
                - self.world.radius(candidate)
                - self.world.radius(entity);
            Some(AiTarget {
                entity: candidate,
                position: target_position,
                distance: gap.max(0.0),
            })
        };

        // The nearest hostile is a separate question from the angriest: it is what pulls an
        // idle enemy that nobody has attacked yet.
        let nearest = self
            .targets
            .nearest_hostile(&self.world, entity)
            .and_then(describe);

        AiPercept {
            position,
            alive,
            target: target.and_then(describe),
            nearest,
            health_fraction,
            wounded_ally,
        }
    }

    /// Uses an instant enemy ability.
    fn execute_enemy_ability(
        &mut self,
        source: EntityId,
        ability: crate::enemy::EnemyAbility,
        target: Option<AiTarget>,
        anchor: Vec3,
    ) {
        let definition = ability.definition();
        // A heal is resolved on whoever it selected and never through the damage path: the
        // shape of a heal is "one ally", and running it through `execute_enemy_shape` would
        // collect everything hostile to the caster, which is precisely the wrong list.
        if let crate::enemy::EnemyEffect::Heal { amount } = definition.effect {
            let Some(target) = target else {
                return;
            };
            if target.distance > definition.range {
                return;
            }
            self.heal_entity(
                source,
                target.entity,
                amount,
                AbilityId::from_enemy(ability),
            );
            return;
        }

        self.events.push(CombatEvent::AbilityUsed {
            source,
            ability: AbilityId::from_enemy(ability),
            targets: 1,
            tick: self.tick,
        });
        match definition.shape {
            TelegraphShape::Single => {
                let Some(target) = target else {
                    return;
                };
                // Re-checked at the moment it lands, not when it was chosen: a player who
                // walked out of range between the decision and the swing is out of range.
                if target.distance > definition.range {
                    return;
                }
                self.deal_damage(
                    source,
                    target.entity,
                    definition.damage,
                    AbilityId::from_enemy(ability),
                    definition.damage_type,
                    false,
                );
            }
            _ => {
                let shape = instant_shape(ability, source, self);
                self.execute_enemy_shape(source, ability, anchor, shape, target);
            }
        }
    }

    /// Resolves an enemy ability that covers an area.
    ///
    /// The shape it hits with is the shape the telegraph announced — the same value, passed
    /// through. That is the whole guarantee behind "the damage area matches the telegraph".
    fn execute_enemy_shape(
        &mut self,
        source: EntityId,
        ability: crate::enemy::EnemyAbility,
        anchor: Vec3,
        shape: TelegraphShape,
        target: Option<AiTarget>,
    ) {
        let definition = ability.definition();
        if !shape.is_area() {
            self.execute_enemy_ability(source, ability, target, anchor);
            return;
        }
        // Burning ground is lit before the impact is resolved, so the pool and the hit that
        // created it are one event pair rather than two things that happened to coincide.
        if let crate::enemy::EnemyEffect::Hazard {
            duration,
            damage_per_second,
        } = definition.effect
        {
            self.light_hazard(
                source,
                ability,
                anchor,
                shape.reach(),
                duration,
                damage_per_second,
            );
        }

        self.hits.clear();
        let mut hits = std::mem::take(&mut self.hits);
        let Some(faction) = self.world.faction(source) else {
            self.hits = hits;
            return;
        };
        for candidate in self.world.entities() {
            if candidate == source || !self.world.is_alive(candidate) {
                continue;
            }
            if !self
                .world
                .faction(candidate)
                .is_some_and(|theirs| faction.is_hostile_to(theirs))
            {
                continue;
            }
            let Some(position) = self.world.position(candidate) else {
                continue;
            };
            if shape.contains(anchor, position, self.world.radius(candidate)) {
                hits.push(candidate);
            }
        }

        self.events.push(CombatEvent::AbilityUsed {
            source,
            ability: AbilityId::from_enemy(ability),
            targets: hits.len() as u32,
            tick: self.tick,
        });
        for victim in &hits {
            self.deal_damage(
                source,
                *victim,
                definition.damage,
                AbilityId::from_enemy(ability),
                definition.damage_type,
                false,
            );
        }
        hits.clear();
        self.hits = hits;
    }

    /// Restores health to one entity and announces it.
    ///
    /// The mirror of [`Simulation::deal_damage`]: the one place an enemy ability puts health
    /// back, so "every health change is observable" holds in both directions.
    fn heal_entity(&mut self, source: EntityId, target: EntityId, amount: f32, ability: AbilityId) {
        if !self.world.is_alive(target) {
            return;
        }
        let restored = self.world.heal(target, amount).applied;
        self.events.push(CombatEvent::AbilityUsed {
            source,
            ability,
            targets: 1,
            tick: self.tick,
        });
        if restored <= 0.0 {
            return;
        }
        self.events.push(CombatEvent::Healing {
            source,
            target,
            amount: restored,
            ability,
            tick: self.tick,
        });
    }

    /// Lights burning ground and announces it as an area effect.
    fn light_hazard(
        &mut self,
        source: EntityId,
        ability: crate::enemy::EnemyAbility,
        anchor: Vec3,
        radius: f32,
        duration: f32,
        damage_per_second: f32,
    ) {
        let mut expired = std::mem::take(&mut self.hazard_expired);
        let id = self.hazards.spawn(
            source,
            ability,
            anchor,
            radius,
            duration,
            damage_per_second,
            &mut expired,
        );
        self.hazard_expired = expired;
        self.flush_expired_hazards();
        // The gameplay radius, carried as the shape a visual is built from: what burns and
        // what is drawn are one number.
        self.events.push(CombatEvent::AreaEffectStarted {
            id,
            source,
            ability: AbilityId::from_enemy(ability),
            position: anchor,
            shape: crate::firemage::zone::ZoneShape::Circle { radius },
            duration,
            tick: self.tick,
        });
    }

    /// Announces every hazard that has stopped burning since the last flush.
    fn flush_expired_hazards(&mut self) {
        let tick = self.tick;
        let mut expired = std::mem::take(&mut self.hazard_expired);
        for (id, position) in expired.drain(..) {
            self.events
                .push(CombatEvent::AreaEffectEnded { id, position, tick });
        }
        self.hazard_expired = expired;
    }

    /// Advances every hazard and applies what it burned.
    ///
    /// Run once a tick from the simulation's own step, after the AI has had its turn, so a
    /// pool lit this tick does not also tick this tick.
    pub(super) fn tick_enemy_hazards(&mut self, step: f32) {
        let mut ticks = std::mem::take(&mut self.hazard_ticks);
        let mut expired = std::mem::take(&mut self.hazard_expired);
        ticks.clear();
        self.hazards.tick(step, &mut ticks, &mut expired);
        self.hazard_expired = expired;

        for hazard in &ticks {
            let Some(faction) = self.world.faction(hazard.owner) else {
                continue;
            };
            let ability = AbilityId::from_enemy(hazard.ability);
            // Collected first: dealing damage can kill, and killing frees a slot the
            // iteration is standing on.
            self.hits.clear();
            let mut hits = std::mem::take(&mut self.hits);
            for candidate in self.world.entities() {
                if !self.world.is_alive(candidate) {
                    continue;
                }
                if !self
                    .world
                    .faction(candidate)
                    .is_some_and(|theirs| faction.is_hostile_to(theirs))
                {
                    continue;
                }
                let Some(position) = self.world.position(candidate) else {
                    continue;
                };
                let inside = {
                    let (dx, dz) = (position.x - hazard.center.x, position.z - hazard.center.z);
                    (dx * dx + dz * dz).sqrt() - self.world.radius(candidate) <= hazard.radius
                };
                if inside {
                    hits.push(candidate);
                }
            }
            for victim in &hits {
                self.deal_damage(
                    hazard.owner,
                    *victim,
                    hazard.damage,
                    ability,
                    hazard.ability.definition().damage_type,
                    false,
                );
            }
            hits.clear();
            self.hits = hits;
        }

        ticks.clear();
        self.hazard_ticks = ticks;
        self.flush_expired_hazards();
    }

    /// Puts out everything an entity lit, and announces every removal.
    pub(super) fn clear_hazards_of(&mut self, owner: EntityId) {
        let mut expired = std::mem::take(&mut self.hazard_expired);
        self.hazards.clear_owner(owner, &mut expired);
        self.hazard_expired = expired;
        self.flush_expired_hazards();
    }

    /// Puts every hazard out. What a reset runs.
    pub(super) fn clear_all_hazards(&mut self) {
        let mut expired = std::mem::take(&mut self.hazard_expired);
        self.hazards.clear_into(&mut expired);
        self.hazard_expired = expired;
        self.flush_expired_hazards();
    }

    /// Announces a phase transition.
    fn announce_phase(&mut self, entity: EntityId, phase: usize, name: &'static str) {
        log::info!("{} entered {name}", entity.index());
        self.events.push(CombatEvent::PhaseChanged {
            entity,
            phase,
            name,
            tick: self.tick,
        });
    }

    /// Announces that an enemy started casting.
    fn announce_cast_start(
        &mut self,
        source: EntityId,
        ability: crate::enemy::EnemyAbility,
        duration: f32,
    ) {
        self.events.push(CombatEvent::CastStarted {
            source,
            ability: AbilityId::from_enemy(ability),
            duration,
            tick: self.tick,
        });
    }

    /// Announces that an enemy's cast ended, however it ended.
    fn announce_cast_end(
        &mut self,
        source: EntityId,
        ability: crate::enemy::EnemyAbility,
        end: CastEnd,
    ) {
        self.events.push(CombatEvent::CastEnded {
            source,
            ability: AbilityId::from_enemy(ability),
            end,
            tick: self.tick,
        });
    }

    /// Announces a telegraph going up.
    fn announce_telegraph(
        &mut self,
        id: u32,
        source: EntityId,
        ability: crate::enemy::EnemyAbility,
        anchor: Vec3,
        shape: TelegraphShape,
        delay: f32,
    ) {
        self.events.push(CombatEvent::TelegraphStarted {
            id,
            source,
            ability: AbilityId::from_enemy(ability),
            anchor,
            shape,
            delay,
            tick: self.tick,
        });
    }

    /// Announces a telegraph coming down.
    fn announce_telegraph_end(&mut self, id: u32, anchor: Vec3, resolved: bool) {
        self.events.push(CombatEvent::TelegraphEnded {
            id,
            anchor,
            resolved,
            tick: self.tick,
        });
    }

    /// The next telegraph identity.
    fn next_telegraph_id(&mut self) -> u32 {
        let id = self.next_telegraph;
        self.next_telegraph = self.next_telegraph.wrapping_add(1);
        id
    }

    /// Interrupts an enemy's cast, if it has one and it can be interrupted.
    ///
    /// Returns whether anything was stopped. This is the whole interrupt foundation: a
    /// player ability that wants to interrupt calls it, and the enemy loses its cast, its
    /// telegraph and a chunk of time.
    pub fn interrupt(&mut self, entity: EntityId) -> bool {
        let slot = entity.index();
        let Some(Some(runtime)) = self.enemies.get_mut(slot) else {
            return false;
        };
        if !runtime.brain.is_interruptible() {
            return false;
        }
        let Some(cast) = runtime.brain.stop_cast(CastStop::Interrupted) else {
            return false;
        };
        let telegraph = runtime.telegraph.take();
        if let Some(id) = telegraph {
            self.announce_telegraph_end(id, cast.anchor, false);
        }
        self.announce_cast_end(entity, cast.ability, CastEnd::Interrupted);
        true
    }

    /// Whether an entity is an enemy with an interruptible cast running.
    #[must_use]
    pub fn is_interruptible(&self, entity: EntityId) -> bool {
        self.enemies
            .get(entity.index())
            .and_then(|slot| slot.as_ref())
            .is_some_and(|runtime| runtime.brain.is_interruptible())
    }

    /// Keeps every encounter's state in step with how many of its members are alive.
    ///
    /// Taken out of `self` and put back so the loop can ask the world questions. A dungeon's
    /// nine encounters are nine cheap passes over a handful of entity ids each.
    fn sync_encounter(&mut self) {
        let mut encounters = std::mem::take(&mut self.encounters);
        for encounter in &mut encounters {
            // Who is awake, and what they are fighting. A pack is pulled *by* something, and
            // the rest of it has to be told what.
            let mut awake = false;
            let mut puller = None;
            for member in encounter.members() {
                let Some(Some(runtime)) = self.enemies.get(member.index()) else {
                    continue;
                };
                // A corpse is not awake and cannot pull anybody. The last member of a pack
                // dying must leave the pack at rest, not permanently pulled.
                if runtime.brain.state() == AiState::Idle || runtime.brain.state() == AiState::Dead
                {
                    continue;
                }
                awake = true;
                puller = puller.or_else(|| runtime.brain.target());
            }
            // A member killed outright never left Idle, so nobody in the pack is awake and
            // there is no live target to read a puller off. Whoever landed the killing blow
            // is that puller: a pack that slept through one of its own being deleted could be
            // taken apart one member at a time.
            let puller = puller.or_else(|| encounter.alarm());
            // One member pulled is the whole pack pulled, once. Done here rather than in the
            // brain because membership is the encounter's: an enemy knows its own aggro
            // radius and its own leash, and has no idea it is the second of five.
            let pull = puller
                .filter(|_| !encounter.pack_pulled())
                .filter(|target| self.world.is_alive(*target));

            if awake {
                encounter.activate();
            } else if pull.is_none() {
                // Everybody is home, or everybody is dead. Either way the pack can be pulled
                // as a whole again if anything is left to pull — and an alarm nobody is left
                // to answer goes quiet with it.
                encounter.relax();
            }

            if let Some(target) = pull {
                encounter.note_pack_pull();
                encounter.activate();
                for member in encounter.members() {
                    // A corpse is not provoked into anything. Its brain has not necessarily
                    // been told it is dead yet — that happens on its next think — and waking
                    // it would make the pack look awake for a tick.
                    if !self.world.is_alive(*member) {
                        continue;
                    }
                    if let Some(Some(runtime)) = self.enemies.get_mut(member.index()) {
                        runtime.brain.provoke(target);
                    }
                }
            }
            let alive = encounter
                .members()
                .iter()
                .filter(|member| self.world.is_alive(**member))
                .count();
            if encounter.observe_alive(alive) {
                log::info!("encounter '{}' completed", encounter.name());
            }
        }
        self.encounters = encounters;
    }
}

/// Who an ability is used on, given what its caster can see.
///
/// The same question [`crate::enemy::brain::EnemyBrain`] asked when it chose the ability, so
/// what the brain aimed at and what combat resolves cannot be two different things.
fn recipient_of(ability: crate::enemy::EnemyAbility, percept: &AiPercept) -> Option<AiTarget> {
    match ability.definition().select {
        crate::enemy::EnemyTarget::Victim => percept.victim(),
        crate::enemy::EnemyTarget::WoundedAlly => percept.wounded_ally,
    }
}

/// Where an instant ability is anchored.
fn cast_anchor(
    ability: crate::enemy::EnemyAbility,
    source: EntityId,
    target: Option<AiTarget>,
    simulation: &Simulation,
) -> Vec3 {
    match ability.definition().aim {
        EnemyAim::Caster => simulation.world().position(source).unwrap_or(Vec3::ZERO),
        EnemyAim::Target => target
            .map(|target| target.position)
            .or_else(|| simulation.world().position(source))
            .unwrap_or(Vec3::ZERO),
    }
}

/// The shape an *instant* ability hits with, aimed the way its caster faces now.
///
/// Casts do not use this: theirs was resolved when the cast started and travels on the cast.
fn instant_shape(
    ability: crate::enemy::EnemyAbility,
    source: EntityId,
    simulation: &Simulation,
) -> TelegraphShape {
    let yaw = simulation
        .world()
        .body(source)
        .map_or(0.0, |body| body.facing_yaw);
    aimed_shape(ability.definition().shape, yaw)
}
