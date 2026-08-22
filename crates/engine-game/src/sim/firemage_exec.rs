//! Casting FireMage: validation, targeting, damage, Ignite, Heat and the fire on the ground.
//!
//! ```text
//! command -> validate -> cast timer -> land -> hit list -> damage + Ignite
//!                                                       -> Heat
//!                                                       -> zones / impacts
//! ```
//!
//! A child module of [`super`] so it can reach the simulation's own state; everything it
//! touches belongs to one class, and keeping it out of `sim/mod.rs` is what stops the loop
//! from being buried in fire.
//!
//! ## What the executor is allowed to decide
//!
//! Which entities are hit, how much damage they take, and what happens to Heat. It never
//! decides that a burn explodes on its own: [`FireAbility::Detonate`] is the only thing in
//! this file that consumes Ignite, and it refuses unless the target is at the current
//! maximum.
//!
//! ## Where the numbers come from
//!
//! [`crate::firemage::Stats`], read at the moment the damage is dealt. Nothing here caches a
//! stat between the cast and the hit, and the burns and zones it creates carry coefficients
//! rather than damage, so a change of Heat or haste is visible on the very next tick.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::firemage::{
    AbilityError, CRIT_FEEDBACK_CAP, FireAbility, IgniteTick, MOLTEN_GROUND_INTERVAL,
    MOLTEN_GROUND_TICK_COEFFICIENT, MOLTEN_GROUND_VULNERABILITY, PendingImpact, TargetMode,
    VULNERABILITY_DURATION, Zone, ZoneKind, ZoneShape, ZoneTick,
};
use crate::hero::Hero;
use crate::sim::CastEnd;

use super::{ActiveCast, Simulation};

/// Half-angle of a cone ability, in radians: a sixty-degree spread, thirty either side.
pub const CONE_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_6;

/// How far off dead ahead a cast may be aimed, in radians.
pub const CAST_FACING_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_2;

/// How much of a burn's remaining damage a detonation converts.
pub const DETONATE_FRACTION: f32 = 0.20;

/// Chance Firebolt adds a stack to an already-burning target.
pub const FIREBOLT_REPEAT_IGNITE_CHANCE: f32 = 0.50;

/// How far a Chain Pyroblast will look for its next victim, in metres.
pub const PYROBLAST_BOUNCE_RANGE: f32 = 15.0;

/// What a cast needs to know about where it was aimed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CastAim {
    /// At whatever is selected.
    Target,
    /// At a point on the ground the player chose.
    Ground(Vec3),
}

/// What the class did this fight, beyond what the damage meter records.
///
/// Everything here is a *count*, not a rule: nothing in gameplay reads these numbers. They
/// exist because the combat simulator has to report cast completion, cooldown resets and
/// Heat cycles, and deriving them from the event stream afterwards would mean a second
/// implementation of the mechanics that produced them.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct FireCounters {
    /// Casts begun, instants included.
    pub casts_started: u32,
    /// Casts that ran to the end.
    pub casts_completed: u32,
    /// Casts that completed against a target that had died in the meantime.
    pub casts_wasted: u32,
    /// Casts cut short by movement, a cancel or death.
    pub casts_interrupted: u32,
    /// Detonations that actually consumed a burn.
    pub detonations: u32,
    /// Ignite stacks applied.
    pub ignite_applications: u32,
    /// Critical strikes dealt, ticks included.
    pub crits: u32,
    /// Damage instances dealt.
    pub hits: u32,
    /// Seconds of cooldown handed back by every reset mechanic.
    pub cooldown_reduction: f32,
    /// Seconds of that reduction that fell on an ability which was not on cooldown.
    pub wasted_cooldown_reduction: f32,
    /// Times Heat reached the top of the bar.
    pub heat_cycles: u32,
    /// Times a Chain Pyroblast bounced.
    pub pyroblast_bounces: u32,
    /// Sum of Heat sampled once per tick, for an average.
    pub heat_sum: f64,
    /// How many samples that sum holds.
    pub heat_samples: u64,
    /// Seconds spent at 30 Heat or more.
    pub seconds_warm: f64,
    /// Seconds spent at 70 Heat or more, the Inferno State included.
    pub seconds_hot: f64,
    /// Seconds the Inferno State ran.
    pub seconds_inferno: f64,
    /// Sum of the Ignite stacks on every burning target, sampled once per tick.
    pub ignite_stack_sum: f64,
    /// How many burning targets those samples covered.
    pub ignite_stack_samples: u64,
}

impl FireCounters {
    /// Average Heat across the fight.
    #[must_use]
    pub fn average_heat(&self) -> f64 {
        if self.heat_samples == 0 {
            return 0.0;
        }
        self.heat_sum / self.heat_samples as f64
    }

    /// Average Ignite stacks on a burning target.
    #[must_use]
    pub fn average_ignite_stacks(&self) -> f64 {
        if self.ignite_stack_samples == 0 {
            return 0.0;
        }
        self.ignite_stack_sum / self.ignite_stack_samples as f64
    }
}

impl Simulation {
    /// What the class did this fight, for the simulator's report.
    #[must_use]
    pub fn fire_counters(&self) -> FireCounters {
        self.fire
    }

    /// Casts a FireMage ability at the current target.
    ///
    /// # Errors
    ///
    /// Returns [`AbilityError`] with the reason it was refused.
    pub fn cast(&mut self, ability: FireAbility) -> Result<u32, AbilityError> {
        self.cast_aimed(ability, CastAim::Target)
    }

    /// Casts a ground-targeted ability at `position`.
    ///
    /// # Errors
    ///
    /// Returns [`AbilityError`] with the reason it was refused.
    pub fn cast_at(&mut self, ability: FireAbility, position: Vec3) -> Result<u32, AbilityError> {
        self.cast_aimed(ability, CastAim::Ground(position))
    }

    /// Casts an ability, recording the refusal on the player if there is one.
    pub fn cast_aimed(&mut self, ability: FireAbility, aim: CastAim) -> Result<u32, AbilityError> {
        let result = self.try_cast(ability, aim);
        match result {
            Ok(_) => {
                self.player.last_error = None;
                self.player.last_ability = Some(AbilityId::from_fire(ability));
            }
            Err(error) => {
                self.player.last_error = Some(error);
                self.player.last_error_at = self.time;
            }
        }
        result
    }

    /// The body of a cast: validate, then either start a cast bar or land at once.
    fn try_cast(&mut self, ability: FireAbility, aim: CastAim) -> Result<u32, AbilityError> {
        let definition = *ability.definition();
        let caster = self.player.entity;

        if ability.is_passive() {
            return Err(AbilityError::OnCooldown);
        }
        // One ability in the game is switched off, and it is Cataclysm. The *slot* beside
        // the bar is not: the Warrior's Last Stand lives there and is a real button.
        if ability.is_disabled() {
            return Err(AbilityError::UltimateDisabled);
        }
        if !self.world.is_alive(caster) {
            return Err(AbilityError::CasterDead);
        }
        if self.player.cast.is_some() {
            self.player.queued = Some(super::QueuedCast {
                ability,
                ground: match aim {
                    CastAim::Ground(position) => Some(position),
                    CastAim::Target => None,
                },
            });
            return Ok(0);
        }
        if !self.mage.abilities.is_ready(ability) {
            return Err(AbilityError::OnCooldown);
        }
        if !self.mage.off_global_cooldown() {
            return Err(AbilityError::GlobalCooldown);
        }
        if definition.energy_cost > 0.0 && !self.mage.energy.is_full() {
            return Err(AbilityError::NotEnoughEnergy);
        }
        self.validate_aim(ability, aim)?;

        // An aimed ability turns the character to where it was aimed. Choosing a direction
        // is how a player says which way they are pointing.
        if let CastAim::Ground(position) = aim {
            self.face_towards(position);
        }

        self.mage.start_global_cooldown();
        self.fire.casts_started += 1;

        let stats = self.mage.stats();
        let cast_time = definition.cast_time_with(&stats);
        if cast_time > 0.0 {
            let ground = match aim {
                CastAim::Ground(position) => Some(position),
                CastAim::Target => None,
            };
            self.player.cast = Some(ActiveCast {
                ability,
                elapsed: 0.0,
                duration: cast_time,
                target: self.player.target,
                ground,
            });
            let tick = self.tick;
            self.events.push(CombatEvent::CastStarted {
                source: caster,
                ability: AbilityId::from_fire(ability),
                duration: cast_time,
                tick,
            });
            return Ok(0);
        }

        self.fire.casts_completed += 1;
        Ok(self.land(ability, aim))
    }

    /// Starts whatever the player queued while the last cast was running.
    pub(super) fn start_queued_cast(&mut self) {
        let Some(queued) = self.player.queued.take() else {
            return;
        };
        let aim = match queued.ground {
            Some(position) => CastAim::Ground(position),
            None => CastAim::Target,
        };
        let _ = self.cast_aimed(queued.ability, aim);
    }

    /// Cancels the cast in progress, if there is one. What escape does.
    pub fn cancel_cast(&mut self) -> bool {
        if self.player.cast.is_none() {
            self.player.queued = None;
            return false;
        }
        self.end_cast(CastEnd::Cancelled);
        true
    }

    /// Whether the player's spells may be pointed at `entity`.
    ///
    /// Hostile to the caster, which the player themselves is not. Selecting yourself is a
    /// legal thing to do — every game in the genre lets a player look at their own frame —
    /// and it must not turn Firebolt into a way to die.
    #[must_use]
    pub fn is_attackable(&self, entity: EntityId) -> bool {
        let caster = self.player.entity;
        if entity == caster {
            return false;
        }
        self.world
            .faction(caster)
            .zip(self.world.faction(entity))
            .is_some_and(|(mine, theirs)| mine.is_hostile_to(theirs))
    }

    /// Whether `point` lies in the caster's frontal half-circle.
    #[must_use]
    pub fn is_facing(&self, point: Vec3) -> bool {
        let caster = self.player.entity;
        let Some(body) = self.world.body(caster) else {
            return false;
        };
        let offset = Vec3::new(point.x - body.position.x, 0.0, point.z - body.position.z);
        let distance = (offset.x * offset.x + offset.z * offset.z).sqrt();
        if distance <= 1e-3 {
            return true;
        }
        let forward = Vec3::new(-body.facing_yaw.sin(), 0.0, -body.facing_yaw.cos());
        let cosine = (offset.x * forward.x + offset.z * forward.z) / distance;
        cosine >= CAST_FACING_HALF_ANGLE.cos()
    }

    /// Turns the character to face `point`.
    ///
    /// Public because a headless driver - the combat simulator, a replay, a future scripted
    /// encounter - has no mouse to turn with, and the facing rule is a real requirement a
    /// targeted cast is refused for.
    pub fn face(&mut self, point: Vec3) {
        self.face_towards(point);
    }

    /// Turns the character to face `point`, if it is anywhere but underfoot.
    pub(super) fn face_towards(&mut self, point: Vec3) {
        let entity = self.player.entity;
        let Some(body) = self.world.body_mut(entity) else {
            return;
        };
        let (dx, dz) = (point.x - body.position.x, point.z - body.position.z);
        if dx.abs() < 1e-4 && dz.abs() < 1e-4 {
            return;
        }
        body.facing_yaw = (-dx).atan2(-dz);
    }

    /// Checks that an ability has something to be cast at.
    ///
    /// `&mut self` for one reason: a line-of-sight check records what it asked and what it
    /// found, so the debug view can show the query that refused the cast rather than a
    /// second one run later against a world that has moved on.
    fn validate_aim(&mut self, ability: FireAbility, aim: CastAim) -> Result<(), AbilityError> {
        let definition = ability.definition();
        let caster = self.player.entity;
        match definition.target {
            TargetMode::Single | TargetMode::TargetCone => {
                let target = self.player.target.ok_or(AbilityError::NoTarget)?;
                if !self.world.is_alive(target) {
                    return Err(AbilityError::TargetInvalid);
                }
                // Selecting yourself is legal; burning yourself is not. Checked here rather
                // than left to the selection rules, because what a target may be *used for*
                // belongs to the ability and a future friendly spell would want the opposite
                // answer from the same selection.
                if !self.is_attackable(target) {
                    return Err(AbilityError::TargetFriendly);
                }
                let distance = self
                    .world
                    .distance_between(caster, target)
                    .ok_or(AbilityError::TargetInvalid)?;
                if distance > definition.range {
                    return Err(AbilityError::OutOfRange);
                }
                let position = self
                    .world
                    .position(target)
                    .ok_or(AbilityError::TargetInvalid)?;
                // A target-pointed cleave *turns* the character towards the selection —
                // choosing a target is how the player says which way the cleave points, and
                // refusing it for facing would be refusing them for the camera.
                if matches!(definition.target, TargetMode::TargetCone) {
                    self.face_towards(position);
                } else if !self.is_facing(position) {
                    return Err(AbilityError::FacingWrongWay);
                }
                // After range, so a target on the far side of the level is still refused
                // for the distance rather than for the wall that also happens to be there.
                // Only an ability that asked for the check casts a ray at all.
                if definition.checks_line_of_sight()
                    && !self.check_line_of_sight(
                        ability,
                        target,
                        crate::sim::LineOfSightPhase::Activation,
                    )
                {
                    return Err(AbilityError::NoLineOfSight);
                }
                // The one requirement that is a *mechanic* rather than a check: Detonate
                // exists to be held until the burn is full.
                if ability == FireAbility::Detonate
                    && !self
                        .mage
                        .ignite
                        .is_at_maximum(target, self.mage.ignite_max())
                {
                    return Err(AbilityError::NotEnoughIgnite);
                }
                Ok(())
            }
            // An aimed ability turns the character towards where it was aimed, so there is
            // no wrong direction to be facing.
            TargetMode::Cone | TargetMode::Line => Ok(()),
            TargetMode::Ground => {
                let CastAim::Ground(position) = aim else {
                    return Err(AbilityError::NeedsGroundTarget);
                };
                let origin = self.world.position(caster).unwrap_or(Vec3::ZERO);
                if horizontal_distance(origin, position) > definition.range {
                    return Err(AbilityError::GroundOutOfRange);
                }
                Ok(())
            }
            TargetMode::SelfCast | TargetMode::Movement => Ok(()),
        }
    }

    /// Applies an ability whose cast has finished.
    ///
    /// A cast that lands on a target that has died since it started simply hits nothing: the
    /// charge is spent, which is what committing to a three-second Pyroblast means.
    pub(super) fn land(&mut self, ability: FireAbility, aim: CastAim) -> u32 {
        let definition = *ability.definition();
        let caster = self.player.entity;
        let effects = *self.mage.effects();
        self.mage.abilities.spend(ability, &effects);
        if definition.energy_cost > 0.0 {
            self.mage.energy.spend_all();
        }
        let origin = self.world.position(caster).unwrap_or(Vec3::ZERO);
        // Read before anything else moves the character: what the cast was aimed at is
        // decided here, and everything the cast produces is pointed the same way.
        let facing = self.aim_yaw(aim, origin);
        let tick = self.tick;

        // Heat is paid once per cast, whatever it hit. Per-target Heat made area abilities
        // reach the top of the bar for free, which is why the old cone needed a cap.
        if definition.heat > 0.0 {
            self.gain_heat(definition.heat);
        }
        self.mage.gain_energy(definition.energy);

        // A ground ability with a delay schedules its impact and stops here.
        if definition.impact_delay > 0.0
            && let CastAim::Ground(position) = aim
        {
            self.mage.zones.schedule(PendingImpact {
                ability,
                position,
                remaining: definition.impact_delay,
                total: definition.impact_delay,
            });
            // Announced when the rock is committed to, not when it arrives: the anticipation
            // an area indicator shows is exactly the delay gameplay is counting down.
            self.events.push(CombatEvent::ImpactScheduled {
                source: caster,
                ability: AbilityId::from_fire(ability),
                position,
                radius: definition.radius,
                delay: definition.impact_delay,
                tick,
            });
            self.events.push(CombatEvent::AbilityUsed {
                source: caster,
                ability: AbilityId::from_fire(ability),
                targets: 0,
                facing,
                tick,
            });
            return 0;
        }

        let hits = self.collect_targets(ability, aim, origin);
        if definition.target == TargetMode::Single && hits == 0 {
            // The cast completed into a corpse. Counted, because a simulator that assumed
            // every completed cast dealt damage would overstate every AoE fight.
            self.fire.casts_wasted += 1;
        }
        self.events.push(CombatEvent::AbilityUsed {
            source: caster,
            ability: AbilityId::from_fire(ability),
            targets: hits as u32,
            facing,
            tick,
        });

        if ability.hero() == Hero::Warrior {
            return self.land_warrior(ability, hits);
        }

        match ability {
            FireAbility::Detonate => self.detonate(),
            FireAbility::FireMastery => {
                self.mage.mastery_buff.start(definition.duration);
                0
            }
            FireAbility::FlameShield => {
                self.mage.flame_shield.start(definition.duration);
                0
            }
            FireAbility::Blink => {
                self.dash(definition.range);
                0
            }
            FireAbility::Firestorm => {
                self.spawn_firestorm(aim, origin);
                0
            }
            FireAbility::Starfall => {
                self.spawn_placed_zone(ability, aim, origin);
                0
            }
            FireAbility::Regrowth => self.heal_self(
                definition.base_damage(&self.mage.stats()),
                FireAbility::Regrowth,
            ),
            _ => self.apply_direct(ability),
        }
    }

    /// Runs a landed impact: Meteor, and the ground a talent leaves behind.
    pub(super) fn resolve_impact(&mut self, impact: PendingImpact) {
        let definition = *impact.ability.definition();
        let effects = *self.mage.effects();
        let stats = self.mage.stats();
        self.hits.clear();
        self.collect_in_shape(
            impact.position,
            ZoneShape::Circle {
                radius: definition.radius,
            },
        );

        // "Per Ignite stack on the primary target": the nearest living enemy to where the
        // rock came down. With nothing under it there is no bonus and nothing to hit.
        let primary_stacks = self
            .nearest_hit_to(impact.position)
            .map_or(0, |target| self.mage.ignite.stacks(target));
        let scaling = 1.0 + f32::from(primary_stacks) * definition.ignite_bonus_per_stack;
        let base = definition.base_damage(&stats) * scaling * effects.meteor_damage_scale;

        let hit_count = self.hits.len() as u32;
        let tick = self.tick;
        self.events.push(CombatEvent::ImpactLanded {
            source: self.player.entity,
            ability: AbilityId::from_fire(impact.ability),
            position: impact.position,
            radius: definition.radius,
            targets: hit_count,
            tick,
        });

        let mut critical = false;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            let crit = self.strike(
                target,
                base,
                impact.ability,
                !effects.meteor_cannot_crit,
                effects.meteor_crit_damage_bonus,
            );
            critical |= crit;
            if definition.ignite_stacks > 0 {
                self.apply_ignite(target, definition.ignite_stacks);
            }
        }

        // Meteoric Impact: a critical rock hurries Pyroblast along.
        if critical && effects.meteor_crit_pyroblast_reduction > 0.0 {
            self.reduce_cooldown(
                FireAbility::Pyroblast,
                effects.meteor_crit_pyroblast_reduction,
                f32::MAX,
            );
        }

        // Molten Meteor: the ground keeps burning whether or not anything stayed in it.
        if impact.ability == FireAbility::Meteor && effects.meteor_ground_duration > 0.0 {
            let shape = ZoneShape::Circle {
                radius: definition.radius,
            };
            let id = self.mage.zones.spawn(Zone {
                kind: ZoneKind::BurningGround,
                source: FireAbility::Meteor,
                position: impact.position,
                shape,
                remaining: effects.meteor_ground_duration,
                until_tick: MOLTEN_GROUND_INTERVAL,
                interval: MOLTEN_GROUND_INTERVAL,
                coefficient: MOLTEN_GROUND_TICK_COEFFICIENT,
                ignite_stacks: 0,
                ignite_chance: 0.0,
            });
            self.events.push(CombatEvent::AreaEffectStarted {
                id,
                source: self.player.entity,
                ability: AbilityId::from_fire(FireAbility::Meteor),
                position: impact.position,
                shape,
                duration: effects.meteor_ground_duration,
                tick,
            });
        }
    }

    /// Runs one tick of a burning zone.
    pub(super) fn resolve_zone_tick(&mut self, tick: ZoneTick) {
        let stats = self.mage.stats();
        let base = tick.coefficient * stats.spell_power;
        self.hits.clear();
        self.collect_in_shape(tick.position, tick.shape);

        for index in 0..self.hits.len() {
            let target = self.hits[index];
            // The debuff first: standing in the fire is what applies it, and it should
            // apply to the tick that put it there.
            if tick.kind.applies_vulnerability() {
                self.mage
                    .vulnerability
                    .apply(target, VULNERABILITY_DURATION);
            }
            self.strike(target, base, tick.source, true, 0.0);
            if tick.ignite_stacks > 0 && self.mage.rng.chance(tick.ignite_chance) {
                self.apply_ignite(target, tick.ignite_stacks);
            }
        }
    }

    /// Applies every burn that ticked this step.
    ///
    /// The damage is computed *here*, from the stats of this moment: the tracker reported
    /// only that a burn with N stacks fired. That is the whole of the no-snapshot rule.
    pub(super) fn resolve_ignite_tick(&mut self, tick: IgniteTick) {
        let Some(target) = self.world.id_at(tick.slot) else {
            return;
        };
        if !self.world.is_alive(target) {
            self.mage.ignite.clear(target);
            return;
        }
        let effects = *self.mage.effects();
        let stats = self.mage.stats();
        let base = f32::from(tick.stacks)
            * crate::firemage::IGNITE_SPELL_POWER_PER_STACK
            * effects.ignite_damage_scale
            * stats.spell_power;

        let crit = self.strike(target, base, FireAbility::Ignite, true, 0.0);

        // Burning Precision: a critical burn can hurry Meteor along.
        if crit
            && effects.ignite_crit_meteor_chance > 0.0
            && self.mage.rng.chance(effects.ignite_crit_meteor_chance)
        {
            self.reduce_cooldown(
                FireAbility::Meteor,
                effects.ignite_crit_meteor_reduction,
                f32::MAX,
            );
        }
    }

    /// The abilities that simply damage what they hit.
    fn apply_direct(&mut self, ability: FireAbility) -> u32 {
        let definition = *ability.definition();
        let effects = *self.mage.effects();
        let stats = self.mage.stats();
        let caster = self.player.entity;
        let origin = self.world.position(caster).unwrap_or(Vec3::ZERO);
        let mut affected = 0;

        for index in 0..self.hits.len() {
            let target = self.hits[index];
            let burning = self.mage.ignite.is_ignited(target);
            let stacks = self.mage.ignite.stacks(target);

            // The line the spell travelled, for whatever presentation wants to fly down it.
            // Emitted before the damage, so an observer reading the batch in order sees the
            // bolt leave before it sees the hit land — even though gameplay resolved both on
            // this tick.
            if definition.target == TargetMode::Single
                && let Some(destination) = self.world.position(target)
            {
                let tick = self.tick;
                self.events.push(CombatEvent::ProjectileLaunched {
                    source: caster,
                    target,
                    from: origin,
                    to: destination,
                    ability: AbilityId::from_fire(ability),
                    tick,
                });
            }

            let scaling = 1.0 + f32::from(stacks) * definition.ignite_bonus_per_stack;
            let base = definition.base_damage(&stats) * scaling;
            let crit = self.strike(target, base, ability, true, 0.0);

            match ability {
                // Firebolt's asymmetry: guaranteed on an unburnt target, a coin flip on a
                // burning one, and never past the maximum.
                FireAbility::Firebolt => {
                    let chance = if burning {
                        FIREBOLT_REPEAT_IGNITE_CHANCE
                    } else {
                        1.0
                    };
                    if self.mage.rng.chance(chance) {
                        self.apply_ignite(target, definition.ignite_stacks);
                    }
                }
                _ if definition.ignite_stacks > 0 => {
                    self.apply_ignite(target, definition.ignite_stacks);
                }
                _ => {}
            }

            if crit {
                match ability {
                    FireAbility::FlameLance => self.reduce_cooldown(
                        FireAbility::Pyroblast,
                        effects.flame_lance_crit_reduction,
                        CRIT_FEEDBACK_CAP,
                    ),
                    FireAbility::Pyroblast => self.reduce_cooldown(
                        FireAbility::FlameLance,
                        effects.pyroblast_crit_reduction,
                        CRIT_FEEDBACK_CAP,
                    ),
                    _ => {}
                }
            }

            if ability == FireAbility::Pyroblast && effects.pyroblast_bounces {
                affected += self.bounce_pyroblast(target, base);
            }
            affected += 1;
        }
        affected
    }

    /// Chain Pyroblast: keep jumping while the crit chance says so.
    ///
    /// The bounce chance is the *current* critical strike chance, read per bounce, so a
    /// chain that starts during Fire Mastery keeps its odds for as long as the buff lasts.
    fn bounce_pyroblast(&mut self, from: EntityId, base: f32) -> u32 {
        let effects = *self.mage.effects();
        let mut chained: Vec<EntityId> = vec![from];
        let mut damage = base;
        let mut previous = from;
        let mut bounces = 0;

        while bounces < effects.pyroblast_max_bounces {
            let chance = self.mage.crit_chance();
            if !self.mage.rng.chance(chance) {
                break;
            }
            let Some(next) =
                self.nearest_living_excluding(previous, PYROBLAST_BOUNCE_RANGE, &chained)
            else {
                break;
            };
            damage *= 1.0 - effects.pyroblast_bounce_falloff;
            self.strike(next, damage, FireAbility::Pyroblast, true, 0.0);
            chained.push(next);
            previous = next;
            bounces += 1;
            self.fire.pyroblast_bounces += 1;
        }
        u32::from(bounces)
    }

    /// Detonate: convert what is left of a full burn into an explosion.
    ///
    /// Every enemy in the radius is treated on its own burn - its own remaining damage, its
    /// own critical strike roll, its own consumption. Nothing is transferred, and an enemy
    /// with no burn takes nothing: the explosion is the burn, not a splash of the primary's
    /// hit.
    fn detonate(&mut self) -> u32 {
        let definition = *FireAbility::Detonate.definition();
        let effects = *self.mage.effects();
        let Some(primary) = self.player.target else {
            return 0;
        };
        let ignite_max = self.mage.ignite_max();
        if !self.mage.ignite.is_at_maximum(primary, ignite_max) {
            return 0;
        }

        let centre = self.world.position(primary).unwrap_or(Vec3::ZERO);
        self.hits.clear();
        self.collect_in_shape(
            centre,
            ZoneShape::Circle {
                radius: definition.radius,
            },
        );
        if !self.hits.contains(&primary) {
            self.hits.push(primary);
        }

        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            let remaining = self.mage.remaining_ignite_damage(target);
            if remaining <= 0.0 {
                continue;
            }
            let consumed = self.mage.ignite.consume(target);
            let mut damage = remaining * DETONATE_FRACTION;
            damage *= 1.0 + f32::from(consumed) * effects.detonate_damage_per_stack;

            // Already multiplied by Heat inside `remaining_ignite_damage`; strike must not
            // apply it twice, so the explosion is dealt with the multiplier divided out.
            let base = damage / self.mage.fire_damage_multiplier();
            self.strike(target, base, FireAbility::Detonate, true, 0.0);
            affected += 1;
        }

        if affected > 0 {
            self.fire.detonations += 1;
        }

        // Rapid Combustion: each area cooldown rolls on its own.
        if effects.detonate_cooldown_chance > 0.0 {
            for ability in [
                FireAbility::FlameCone,
                FireAbility::Firestorm,
                FireAbility::Meteor,
            ] {
                if self.mage.rng.chance(effects.detonate_cooldown_chance) {
                    self.reduce_cooldown(ability, effects.detonate_cooldown_reduction, f32::MAX);
                }
            }
        }
        affected
    }

    /// Moves the caster `distance` metres forward: as far as the geometry allows, and no
    /// further.
    ///
    /// One movement for both of the abilities that are movements. A Blink and a Charge
    /// differ in how far they go and in what the presentation layer makes of them; they do
    /// not differ in what happens to the character, and two copies of "slide forward and
    /// forget the interpolation history" is two chances to fix a teleport bug once.
    fn dash(&mut self, distance: f32) {
        let entity = self.player.entity;
        let Some(body) = self.world.body(entity).copied() else {
            return;
        };
        let yaw = body.facing_yaw;
        let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        let from = body.position;
        self.slide_player(forward * distance);
        let Some(to) = self.world.position(entity) else {
            return;
        };

        // The mage arrives; it does not travel. Without forgetting last tick's position the
        // presentation layer interpolates across the gap and the character slides the whole
        // way over one frame, which is the one thing a teleport must not look like.
        self.world.reset_history(entity);
        let tick = self.tick;
        self.events.push(CombatEvent::Teleported {
            entity,
            from,
            to,
            tick,
        });
    }

    /// Heals the caster and announces it. Returns how many entities were affected.
    ///
    /// The only heal a player can cast. It goes through [`GameWorld::heal`] and the
    /// `Healing` event exactly as an enemy's does, so "every health change is observable"
    /// holds for the player's own as well.
    pub(super) fn heal_self(&mut self, amount: f32, ability: FireAbility) -> u32 {
        let entity = self.player.entity;
        if !self.world.is_alive(entity) {
            return 0;
        }
        let restored = self.world.heal(entity, amount).applied;
        if restored <= 0.0 {
            return 0;
        }
        let tick = self.tick;
        self.events.push(CombatEvent::Healing {
            source: entity,
            target: entity,
            amount: restored,
            ability: AbilityId::from_fire(ability),
            tick,
        });
        1
    }

    /// Places a circular zone where the player put it: Starfall's patch of sky.
    ///
    /// The same [`Zone`] Firestorm and Molten Meteor's ground are, with the shape and the
    /// numbers off the ability's own row. What burns, how often and for how long is data;
    /// the only thing this decides is where the circle goes.
    fn spawn_placed_zone(&mut self, ability: FireAbility, aim: CastAim, origin: Vec3) {
        let definition = *ability.definition();
        let position = match aim {
            CastAim::Ground(position) => position,
            CastAim::Target => origin,
        };
        let shape = ZoneShape::Circle {
            radius: definition.radius,
        };
        let id = self.mage.zones.spawn(Zone {
            kind: ZoneKind::Starfall,
            source: ability,
            position,
            shape,
            remaining: definition.duration,
            until_tick: definition.tick_interval,
            interval: definition.tick_interval,
            coefficient: definition.tick_coefficient,
            ignite_stacks: 0,
            ignite_chance: 0.0,
        });
        let tick = self.tick;
        self.events.push(CombatEvent::AreaEffectStarted {
            id,
            source: self.player.entity,
            ability: AbilityId::from_fire(ability),
            position,
            shape,
            duration: definition.duration,
            tick,
        });
    }

    /// Places Firestorm's line of fire, anchored on the caster and aimed where it was cast.
    fn spawn_firestorm(&mut self, aim: CastAim, origin: Vec3) {
        let definition = *FireAbility::Firestorm.definition();
        let yaw = self.aim_yaw(aim, origin);
        let shape = ZoneShape::Line {
            yaw,
            length: definition.range,
            width: definition.width,
        };
        let id = self.mage.zones.spawn(Zone {
            kind: ZoneKind::Firestorm,
            source: FireAbility::Firestorm,
            position: origin,
            shape,
            remaining: definition.duration,
            until_tick: definition.tick_interval,
            interval: definition.tick_interval,
            coefficient: definition.tick_coefficient,
            ignite_stacks: definition.ignite_stacks,
            ignite_chance: definition.ignite_chance,
        });
        // The shape carried is the shape the containment test uses, so a band of fire drawn
        // from this cannot claim a different footprint from the one that burns.
        let tick = self.tick;
        self.events.push(CombatEvent::AreaEffectStarted {
            id,
            source: self.player.entity,
            ability: AbilityId::from_fire(FireAbility::Firestorm),
            position: origin,
            shape,
            duration: definition.duration,
            tick,
        });
    }

    /// Which way a cast was aimed: towards the point it was placed at, or dead ahead.
    ///
    /// The one answer every part of a cast uses - the hit list, the zone it leaves and the
    /// effect drawn for it - so a cone cannot burn one wedge and be painted over another.
    /// Read once, when the ability lands, because the character turns again the moment the
    /// camera does.
    fn aim_yaw(&self, aim: CastAim, origin: Vec3) -> f32 {
        match aim {
            CastAim::Ground(position) => {
                let (dx, dz) = (position.x - origin.x, position.z - origin.z);
                if dx.abs() > 1e-4 || dz.abs() > 1e-4 {
                    (-dx).atan2(-dz)
                } else {
                    self.facing_yaw()
                }
            }
            CastAim::Target => self.facing_yaw(),
        }
    }

    /// Which way the character is pointing.
    fn facing_yaw(&self) -> f32 {
        self.world
            .body(self.player.entity)
            .map_or(0.0, |body| body.facing_yaw)
    }

    /// Fills the hit list with whatever an ability affects.
    fn collect_targets(&mut self, ability: FireAbility, aim: CastAim, origin: Vec3) -> usize {
        let definition = ability.definition();
        self.hits.clear();
        match definition.target {
            TargetMode::Single => {
                // Alive *and* an enemy: `try_cast` refused a friendly target already, and
                // this is the second of the two places that must agree about who a
                // single-target spell lands on.
                if let Some(target) = self.player.target
                    && self.world.is_alive(target)
                    && self.is_attackable(target)
                {
                    self.hits.push(target);
                }
            }
            TargetMode::Cone => {
                let yaw = self.aim_yaw(aim, origin);
                self.collect_in_cone(origin, yaw, definition.range, CONE_HALF_ANGLE);
            }
            TargetMode::TargetCone => {
                // Pointed by the selection rather than by the mouse, and wider: see
                // `TARGET_CLEAVE_HALF_ANGLE`. Everything else is the same cone.
                let yaw = match self
                    .player
                    .target
                    .and_then(|target| self.world.position(target))
                {
                    Some(position) => crate::enemy::brain::yaw_towards(origin, position),
                    None => self.facing_yaw(),
                };
                self.face_towards(origin + Vec3::new(-yaw.sin(), 0.0, -yaw.cos()));
                self.collect_in_cone(
                    origin,
                    yaw,
                    definition.range,
                    crate::firemage::TARGET_CLEAVE_HALF_ANGLE,
                );
            }
            TargetMode::Ground => {
                if let CastAim::Ground(position) = aim {
                    self.collect_in_shape(
                        position,
                        ZoneShape::Circle {
                            radius: definition.radius,
                        },
                    );
                }
            }
            // A self-cast with a radius is an area centred on the caster: Colossus Strike
            // smashes the ground it is standing on. One without is a buff and hits nobody.
            TargetMode::SelfCast if definition.radius > 0.0 => {
                self.collect_in_shape(
                    origin,
                    ZoneShape::Circle {
                        radius: definition.radius,
                    },
                );
            }
            // Firestorm is placed as a zone rather than hitting on the cast.
            TargetMode::Line | TargetMode::SelfCast | TargetMode::Movement => {}
        }
        self.hits.len()
    }

    /// Fills the hit list with every living hostile inside `shape` anchored at `anchor`.
    pub(super) fn collect_in_shape(&mut self, anchor: Vec3, shape: ZoneShape) {
        let player = self.player.entity;
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if id == player || !self.world.is_alive(id) {
                continue;
            }
            let Some(position) = self.world.position(id) else {
                continue;
            };
            if shape.contains(anchor, position, self.world.radius(id)) {
                self.hits.push(id);
            }
        }
    }

    /// Fills the hit list with every living hostile inside a cone.
    pub(super) fn collect_in_cone(&mut self, origin: Vec3, yaw: f32, range: f32, half_angle: f32) {
        let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        let player = self.player.entity;
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if id == player || !self.world.is_alive(id) {
                continue;
            }
            let Some(position) = self.world.position(id) else {
                continue;
            };
            let offset = Vec3::new(position.x - origin.x, 0.0, position.z - origin.z);
            let distance = (offset.x * offset.x + offset.z * offset.z).sqrt();
            if distance > range + self.world.radius(id) {
                continue;
            }
            if distance <= 0.001 {
                self.hits.push(id);
                continue;
            }
            let cosine = (offset.x * forward.x + offset.z * forward.z) / distance;
            if cosine >= half_angle.cos() {
                self.hits.push(id);
            }
        }
    }

    /// The entity in the current hit list nearest to `point`.
    fn nearest_hit_to(&self, point: Vec3) -> Option<EntityId> {
        let mut best: Option<(f32, EntityId)> = None;
        for id in &self.hits {
            let Some(position) = self.world.position(*id) else {
                continue;
            };
            let distance = horizontal_distance(point, position);
            if best.is_none_or(|(current, _)| distance < current) {
                best = Some((distance, *id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The nearest living hostile to `from` that is not already in `chained`.
    fn nearest_living_excluding(
        &self,
        from: EntityId,
        range: f32,
        chained: &[EntityId],
    ) -> Option<EntityId> {
        let origin = self.world.position(from)?;
        let player = self.player.entity;
        let mut best: Option<(f32, EntityId)> = None;
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if id == player || !self.world.is_alive(id) || chained.contains(&id) {
                continue;
            }
            let Some(position) = self.world.position(id) else {
                continue;
            };
            let distance = horizontal_distance(origin, position);
            if distance > range {
                continue;
            }
            if best.is_none_or(|(current, _)| distance < current) {
                best = Some((distance, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Deals one instance of fire damage, from `base` and the stats of this moment.
    ///
    /// The single place a FireMage number becomes damage: Heat's band, the target's fire
    /// vulnerability and the critical strike all apply here, so a new source of damage
    /// cannot forget one of them. Returns whether it critically struck.
    pub(super) fn strike(
        &mut self,
        target: EntityId,
        base: f32,
        ability: FireAbility,
        allow_crit: bool,
        crit_damage_bonus: f32,
    ) -> bool {
        let stats = self.mage.stats();
        let mut amount = base
            * self.mage.fire_damage_multiplier()
            * self
                .mage
                .vulnerability
                .multiplier(target, MOLTEN_GROUND_VULNERABILITY);

        let crit = allow_crit && self.mage.roll_crit();
        if crit {
            amount *= stats.crit_damage + crit_damage_bonus;
            self.fire.crits += 1;
        }
        self.fire.hits += 1;

        self.deal_damage(
            self.player.entity,
            target,
            amount,
            AbilityId::from_fire(ability),
            ability.definition().damage_type,
            crit,
        );
        crit
    }

    /// Applies Ignite, respecting the current maximum.
    fn apply_ignite(&mut self, target: EntityId, stacks: u8) {
        if stacks == 0 || !self.world.is_alive(target) {
            return;
        }
        let maximum = self.mage.ignite_max();
        let duration = self.mage.ignite_duration();
        let applied = self.mage.ignite.apply(target, stacks, maximum, duration);
        if applied > 0 {
            self.fire.ignite_applications += u32::from(applied);
            let tick = self.tick;
            self.events.push(CombatEvent::StatusApplied {
                source: self.player.entity,
                target,
                status: crate::status::StatusKind::Burning,
                duration,
                tick,
            });
        }
    }

    /// Hands `ability` some of its cooldown back, never more than `cap` at once.
    ///
    /// The cap is the design's hard ceiling on the critical strike feedback loop - 1.5 s and
    /// not a millisecond more, whatever a talent claims. What the reduction could not use is
    /// recorded rather than banked, because a reduction that fell on a ready ability is a
    /// real loss the simulator has to be able to see.
    pub(super) fn reduce_cooldown(&mut self, ability: FireAbility, seconds: f32, cap: f32) {
        let seconds = seconds.min(cap);
        if seconds <= 0.0 {
            return;
        }
        let effects = *self.mage.effects();
        let wasted = self
            .mage
            .abilities
            .reduce_cooldown(ability, seconds, &effects);
        self.fire.cooldown_reduction += seconds - wasted;
        self.fire.wasted_cooldown_reduction += wasted;
    }

    /// Adds Heat and counts the cycle if it reached the top of the bar.
    pub(super) fn gain_heat(&mut self, amount: f32) {
        if self.mage.gain_heat(amount) {
            self.fire.heat_cycles += 1;
        }
    }
}

/// Distance on the ground, ignoring height.
#[must_use]
pub fn horizontal_distance(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}
