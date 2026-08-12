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
            match request {
                AiRequest::None => {}
                AiRequest::Instant(ability) => {
                    let victim = percept.victim();
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
                    self.execute_enemy_shape(entity, ability, anchor, shape, percept.victim());
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

    /// What one enemy can see this tick.
    fn perceive(&mut self, entity: EntityId, brain: &EnemyBrain) -> AiPercept {
        let position = self.world.position(entity).unwrap_or(Vec3::ZERO);
        let alive = self.world.is_alive(entity);

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

    /// Keeps the encounter's state in step with how many of its members are alive.
    fn sync_encounter(&mut self) {
        let Some(mut encounter) = self.encounter.take() else {
            return;
        };
        let pulled = encounter.members().iter().any(|member| {
            self.enemies
                .get(member.index())
                .and_then(|slot| slot.as_ref())
                .is_some_and(|runtime| runtime.brain.state() != AiState::Idle)
        });
        if pulled {
            encounter.activate();
        }
        let alive = encounter
            .members()
            .iter()
            .filter(|member| self.world.is_alive(**member))
            .count();
        if encounter.observe_alive(alive) {
            log::info!("encounter '{}' completed", encounter.name());
        }
        self.encounter = Some(encounter);
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
