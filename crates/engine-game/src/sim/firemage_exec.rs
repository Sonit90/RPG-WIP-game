//! Casting FireMage: validation, targeting, damage, Ignite, Heat and the fire on the ground.
//!
//! ```text
//! command -> validate -> pay charge -> cast timer -> land -> hit list -> damage + Ignite
//!                                                                    -> Heat + Energy
//!                                                                    -> zones / impacts
//! ```
//!
//! A child module of [`super`] so it can reach the simulation's own state; everything it
//! touches belongs to one class, and keeping it out of `sim/mod.rs` is what stops the loop
//! from being buried in fire.
//!
//! ## What the executor is allowed to decide
//!
//! Which entities are hit, how much damage they take, and what happens to the resources.
//! It never decides that a burn explodes on its own: [`FireAbility::FireDetonation`] is the
//! only thing in this file that consumes Ignite, and it refuses unless the target is at the
//! current maximum.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::combat::DamageType;
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::firemage::{
    AbilityError, FireAbility, INFERNAL_GROUND_SLOW, INFERNO_CRIT_DURATION, IgniteTick,
    PYROCLASM_INTERNAL_COOLDOWN, PendingImpact, TargetMode, Zone, ZoneKind, ZoneTick,
};
use crate::sim::CastEnd;

use super::{ActiveCast, Simulation};

/// Half-angle of a cone ability, in radians. 60 degrees either side of where you face.
pub const CONE_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_3;

/// How much of a detonation's damage its splash deals to secondary targets.
pub const DETONATION_SPLASH_FRACTION: f32 = 0.6;

/// Extra damage a detonation deals per Ignite stack consumed.
pub const DETONATION_DAMAGE_PER_STACK: f32 = 120.0;

/// Extra damage Flame Lance deals against a burning target.
pub const FLAME_LANCE_IGNITE_BONUS: f32 = 0.40;

/// Extra damage Pyroblast deals per Ignite stack on the target.
pub const PYROBLAST_BONUS_PER_STACK: f32 = 0.10;

/// Extra damage a burning target takes from Firestorm.
pub const FIRESTORM_IGNITE_BONUS: f32 = 0.15;

/// Chance Firebolt adds a stack to an already-burning target.
pub const FIREBOLT_REPEAT_IGNITE_CHANCE: f32 = 0.50;

/// Chance a Firestorm tick applies Ignite.
pub const FIRESTORM_IGNITE_CHANCE: f32 = 0.20;

/// What a cast needs to know about where it was aimed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CastAim {
    /// At whatever is selected.
    Target,
    /// At a point on the ground the player chose.
    Ground(Vec3),
}

impl Simulation {
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
            Err(error) => self.player.last_error = Some(error),
        }
        result
    }

    /// The body of a cast: validate, pay, then either start a cast bar or land at once.
    fn try_cast(&mut self, ability: FireAbility, aim: CastAim) -> Result<u32, AbilityError> {
        let definition = *ability.definition();
        let caster = self.player.entity;

        if !self.world.is_alive(caster) {
            return Err(AbilityError::CasterDead);
        }
        if !self.mage.abilities.is_ready(ability) {
            return Err(AbilityError::OnCooldown);
        }
        if definition.energy_cost > 0.0 && !self.mage.energy.is_full() {
            return Err(AbilityError::NotEnoughEnergy);
        }
        self.validate_aim(ability, aim)?;

        // Everything is paid at the start of the cast, which is what makes an interrupted
        // cast a real loss rather than a free look at the target's health bar.
        let effects = *self.mage.effects();
        self.mage.abilities.spend(ability, &effects);
        if definition.energy_cost > 0.0 {
            self.mage.energy.spend_all();
            // Arcane Pressure is paid out of the same bar, so spending it resets the bonus.
        }

        let cast_time = definition.cast_time_with(&effects, self.mage.haste());
        if cast_time > 0.0 {
            if self.player.cast.is_some() {
                self.end_cast(CastEnd::Replaced);
            }
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

        // An instant ability interrupts a cast: the player asked for something else.
        if self.player.cast.is_some() {
            self.end_cast(CastEnd::Replaced);
        }
        Ok(self.land(ability, aim))
    }

    /// Checks that an ability has something to be cast at.
    fn validate_aim(&self, ability: FireAbility, aim: CastAim) -> Result<(), AbilityError> {
        let definition = ability.definition();
        let caster = self.player.entity;
        match definition.target {
            TargetMode::Single => {
                let target = self.player.target.ok_or(AbilityError::NoTarget)?;
                if !self.world.is_alive(target) {
                    return Err(AbilityError::TargetInvalid);
                }
                let distance = self
                    .world
                    .distance_between(caster, target)
                    .ok_or(AbilityError::TargetInvalid)?;
                if distance > definition.range {
                    return Err(AbilityError::OutOfRange);
                }
                // The one requirement that is a *mechanic* rather than a check: Fire
                // Detonation exists to be held until the burn is full.
                if ability == FireAbility::FireDetonation
                    && !self
                        .mage
                        .ignite
                        .is_at_maximum(target, self.mage.ignite_max())
                {
                    return Err(AbilityError::NotEnoughIgnite);
                }
                Ok(())
            }
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
            TargetMode::Cone
            | TargetMode::PointBlank
            | TargetMode::SelfCast
            | TargetMode::Movement => Ok(()),
        }
    }

    /// Applies an ability whose cost has already been paid.
    ///
    /// Called immediately for an instant ability and by the cast bar for a slow one. A cast
    /// that lands on a target that has died since it started simply hits nothing: the cost
    /// is spent, which is what committing to a three-second Pyroblast means.
    pub(super) fn land(&mut self, ability: FireAbility, aim: CastAim) -> u32 {
        let definition = *ability.definition();
        let caster = self.player.entity;
        let origin = self.world.position(caster).unwrap_or(Vec3::ZERO);
        let tick = self.tick;

        // A ground ability with a delay schedules its impact and stops here; the rest of
        // this function is what happens when something actually lands.
        if definition.impact_delay > 0.0
            && let CastAim::Ground(position) = aim
        {
            self.mage.zones.schedule(PendingImpact {
                ability,
                position,
                remaining: definition.impact_delay,
                total: definition.impact_delay,
            });
            self.gain_cast_resources(ability, 1);
            self.events.push(CombatEvent::AbilityUsed {
                source: caster,
                ability: AbilityId::from_fire(ability),
                targets: 0,
                tick,
            });
            return 0;
        }

        let hits = self.collect_targets(ability, aim, origin);
        self.events.push(CombatEvent::AbilityUsed {
            source: caster,
            ability: AbilityId::from_fire(ability),
            targets: hits as u32,
            tick,
        });

        let affected = match ability {
            FireAbility::FireDetonation => self.detonate(),
            FireAbility::FlameShield => {
                self.mage.flame_shield.start(definition.duration);
                0
            }
            FireAbility::Blink => {
                self.blink();
                0
            }
            FireAbility::Firestorm | FireAbility::Inferno => {
                self.spawn_ability_zone(ability, aim, origin);
                0
            }
            _ => self.apply_direct(ability, aim),
        };

        self.gain_cast_resources(ability, affected.max(1));
        affected
    }

    /// Runs a landed impact: Meteor, Cataclysm, and the ground fire they leave.
    pub(super) fn resolve_impact(&mut self, impact: PendingImpact) {
        let definition = *impact.ability.definition();
        let effects = *self.mage.effects();
        self.hits.clear();
        self.collect_in_radius(impact.position, definition.radius);

        let ignite_max = self.mage.ignite_max();
        let mut hit_full_burn = false;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            if impact.ability == FireAbility::Meteor
                && self.mage.ignite.is_at_maximum(target, ignite_max)
            {
                hit_full_burn = true;
            }
            let (amount, crit) = self.roll_fire_damage(definition.damage);
            self.deal_fire_damage(target, amount, impact.ability, crit);
            self.apply_ignite(target, definition.ignite_stacks);
        }

        // Meteoric Collapse: a rock landing on a full burn hurries Pyroblast along.
        if hit_full_burn && effects.meteor_max_stack_pyroblast_refund > 0.0 {
            self.mage.abilities.reduce_cooldown(
                FireAbility::Pyroblast,
                effects.meteor_max_stack_pyroblast_refund,
                &effects,
            );
        }

        match impact.ability {
            FireAbility::Meteor if effects.meteor_ground_duration > 0.0 => {
                self.spawn_zone(Zone {
                    kind: ZoneKind::BurningGround,
                    source: FireAbility::Meteor,
                    position: impact.position,
                    radius: definition.radius,
                    remaining: effects.meteor_ground_duration,
                    until_tick: 1.0,
                    interval: 1.0,
                    damage: definition.damage * 0.15,
                    ignite_stacks: 0,
                    ignite_chance: 0.0,
                });
            }
            FireAbility::Cataclysm => {
                // Phase two: the ground keeps burning whether or not anything stayed in it.
                self.mage.cataclysm_active = true;
                self.spawn_zone(Zone {
                    kind: ZoneKind::InfernalGround,
                    source: FireAbility::Cataclysm,
                    position: impact.position,
                    radius: definition.radius,
                    remaining: definition.duration,
                    until_tick: definition.tick_interval,
                    interval: definition.tick_interval,
                    damage: definition.tick_damage,
                    ignite_stacks: 1,
                    ignite_chance: 1.0,
                });
            }
            _ => {}
        }
    }

    /// Runs one tick of a burning zone.
    pub(super) fn resolve_zone_tick(&mut self, tick: ZoneTick) {
        self.hits.clear();
        self.collect_in_radius(tick.position, tick.radius);
        let ignite_max = self.mage.ignite_max();
        let mut touched_full_burn = false;

        for index in 0..self.hits.len() {
            let target = self.hits[index];
            let burning = self.mage.ignite.is_ignited(target);
            if self.mage.ignite.is_at_maximum(target, ignite_max) {
                touched_full_burn = true;
            }
            // Firestorm punishes what is already burning; the brief's +15%.
            let scale = if tick.kind == ZoneKind::Firestorm && burning {
                1.0 + FIRESTORM_IGNITE_BONUS
            } else {
                1.0
            };
            let (amount, crit) = self.roll_fire_damage(tick.damage * scale);
            self.deal_fire_damage(target, amount, tick.source, crit);

            if tick.ignite_stacks > 0 && self.mage.rng.chance(tick.ignite_chance) {
                self.apply_ignite(target, tick.ignite_stacks);
            }
            if tick.kind.slows() {
                self.apply_slow(target);
            }
        }

        // Inferno's critical strike bonus is checked every tick: it is about standing next
        // to a full burn, not about the moment the ability was pressed.
        if tick.kind == ZoneKind::Inferno && touched_full_burn {
            self.mage.inferno_crit.start(INFERNO_CRIT_DURATION);
        }

        self.gain_zone_heat(tick.kind, tick.source);
    }

    /// Applies every burn that ticked this step.
    pub(super) fn resolve_ignite_tick(&mut self, tick: IgniteTick) {
        let Some(target) = self.world.id_at(tick.slot) else {
            return;
        };
        if !self.world.is_alive(target) {
            self.mage.ignite.clear(target);
            return;
        }
        let amount = tick.damage * self.mage.fire_damage_multiplier();
        self.deal_damage(
            self.player.entity,
            target,
            amount,
            AbilityId::from_fire(FireAbility::Firebolt),
            DamageType::Fire,
        );
    }

    /// The abilities that simply damage what they hit.
    fn apply_direct(&mut self, ability: FireAbility, _aim: CastAim) -> u32 {
        let definition = *ability.definition();
        let effects = *self.mage.effects();
        let ignite_max = self.mage.ignite_max();
        let mut affected = 0;

        for index in 0..self.hits.len() {
            let target = self.hits[index];
            let burning = self.mage.ignite.is_ignited(target);
            let stacks = self.mage.ignite.stacks(target);
            let at_maximum = self.mage.ignite.is_at_maximum(target, ignite_max);

            let mut base = definition.damage;
            let mut force_crit = false;
            match ability {
                FireAbility::FlameLance if burning => base *= 1.0 + FLAME_LANCE_IGNITE_BONUS,
                FireAbility::Pyroblast => {
                    base *= 1.0 + f32::from(stacks) * PYROBLAST_BONUS_PER_STACK;
                    if at_maximum {
                        base *= 1.0 + effects.pyroblast_max_stack_bonus;
                        force_crit = true;
                    }
                }
                _ => {}
            }

            if force_crit {
                self.mage.guaranteed_crit = true;
            }
            let (amount, crit) = self.roll_fire_damage(base);
            self.deal_fire_damage(target, amount, ability, crit);

            // Firebolt's Ignite rule, exactly as designed: always on an unburnt target, a
            // coin flip on a burning one, and never past the maximum.
            match ability {
                FireAbility::Firebolt => {
                    let chance = if burning {
                        FIREBOLT_REPEAT_IGNITE_CHANCE + effects.firebolt_extra_ignite_chance
                    } else {
                        1.0
                    };
                    if self.mage.rng.chance(chance) {
                        self.apply_ignite(target, 1);
                    }
                    if crit && effects.crit_firebolt_bonus_energy > 0.0 {
                        self.mage.gain_energy(effects.crit_firebolt_bonus_energy);
                    }
                }
                _ if definition.ignite_stacks > 0 => {
                    self.apply_ignite(target, definition.ignite_stacks);
                }
                _ => {}
            }

            // Critical Ignition: any critical fire spell can add a stack, cap respected.
            if crit && self.mage.rng.chance(effects.critical_ignition_chance) {
                self.apply_ignite(target, 1);
            }
            affected += 1;
        }

        // Pyroclasm arms the next spell, on its own internal cooldown.
        if ability == FireAbility::Pyroblast
            && effects.pyroclasm
            && !self.mage.pyroclasm_cooldown.is_active()
        {
            self.mage.guaranteed_crit = true;
            self.mage
                .pyroclasm_cooldown
                .start(PYROCLASM_INTERNAL_COOLDOWN);
        }
        affected
    }

    /// Fire Detonation: consume a full burn, explode, splash, and never fire on its own.
    fn detonate(&mut self) -> u32 {
        let definition = *FireAbility::FireDetonation.definition();
        let effects = *self.mage.effects();
        let Some(target) = self.player.target else {
            return 0;
        };
        let ignite_max = self.mage.ignite_max();
        if !self.mage.ignite.is_at_maximum(target, ignite_max) {
            return 0;
        }

        let consumed = self.mage.ignite.consume(
            target,
            effects.detonation_retained_stacks,
            self.mage.ignite_duration(),
        );

        // The explosion scales with what it ate, and with the talents that reward eating a
        // full burn.
        let mut damage = definition.damage + f32::from(consumed) * DETONATION_DAMAGE_PER_STACK;
        damage *= 1.0 + f32::from(consumed) * effects.detonation_damage_per_stack;
        if consumed >= ignite_max {
            damage *= 1.0 + effects.detonation_max_stack_bonus;
        }

        let (amount, crit) = self.roll_fire_damage(damage);
        let killed_before = self.world.is_alive(target);
        self.deal_fire_damage(target, amount, FireAbility::FireDetonation, crit);
        let killed = killed_before && !self.world.is_alive(target);

        // The splash: everything else within ten metres of the target.
        let centre = self.world.position(target).unwrap_or(Vec3::ZERO);
        self.hits.clear();
        self.collect_in_radius(centre, definition.radius);
        let mut splash_damage = damage * DETONATION_SPLASH_FRACTION;
        if self.mage.cataclysm_active {
            splash_damage *= 1.0 + effects.detonation_cataclysm_splash;
        }
        let secondary_stacks = definition.ignite_stacks + effects.detonation_extra_secondary_stacks;

        let mut affected = 1;
        for index in 0..self.hits.len() {
            let other = self.hits[index];
            if other == target {
                continue;
            }
            let (amount, crit) = self.roll_fire_damage(splash_damage);
            self.deal_fire_damage(other, amount, FireAbility::FireDetonation, crit);
            self.apply_ignite(other, secondary_stacks);
            affected += 1;
        }

        if killed && effects.detonation_kill_meteor_refund > 0.0 {
            self.mage.abilities.reduce_cooldown(
                FireAbility::Meteor,
                effects.detonation_kill_meteor_refund,
                &effects,
            );
        }
        if effects.detonation_bonus_energy > 0.0 {
            self.mage.gain_energy(effects.detonation_bonus_energy);
        }
        affected
    }

    /// Blink: as far forward as the geometry allows, and no further.
    ///
    /// The collided move is the same one walking uses, so a Blink can climb a stair and
    /// cannot pass a wall - and a blocked Blink lands against the obstacle rather than
    /// failing, which is what the brief asks for and what a player expects.
    fn blink(&mut self) {
        let distance = FireAbility::Blink.definition().range;
        let entity = self.player.entity;
        let Some(body) = self.world.body(entity).copied() else {
            return;
        };
        let yaw = body.facing_yaw;
        let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        self.slide_player(forward * distance);
    }

    /// Places the zone an ability leaves behind.
    fn spawn_ability_zone(&mut self, ability: FireAbility, aim: CastAim, origin: Vec3) {
        let definition = *ability.definition();
        let (kind, position, chance) = match ability {
            FireAbility::Firestorm => {
                let CastAim::Ground(position) = aim else {
                    return;
                };
                (ZoneKind::Firestorm, position, FIRESTORM_IGNITE_CHANCE)
            }
            FireAbility::Inferno => (ZoneKind::Inferno, origin, 1.0),
            _ => return,
        };
        self.mage.firestorm_heat = 0.0;
        self.mage.inferno_extension = 0.0;
        self.spawn_zone(Zone {
            kind,
            source: ability,
            position,
            radius: definition.radius,
            remaining: definition.duration,
            until_tick: definition.tick_interval,
            interval: definition.tick_interval,
            damage: definition.tick_damage,
            ignite_stacks: definition.ignite_stacks,
            ignite_chance: chance,
        });
    }

    /// Adds a zone and announces it.
    fn spawn_zone(&mut self, zone: Zone) {
        self.mage.zones.spawn(zone);
    }

    /// Fills the hit list with whatever an ability affects.
    fn collect_targets(&mut self, ability: FireAbility, aim: CastAim, origin: Vec3) -> usize {
        let definition = ability.definition();
        self.hits.clear();
        match definition.target {
            TargetMode::Single => {
                if let Some(target) = self.player.target
                    && self.world.is_alive(target)
                {
                    self.hits.push(target);
                }
            }
            TargetMode::Cone => {
                let yaw = self
                    .world
                    .body(self.player.entity)
                    .map_or(0.0, |body| body.facing_yaw);
                self.collect_in_cone(origin, yaw, definition.range);
            }
            TargetMode::Ground => {
                if let CastAim::Ground(position) = aim {
                    self.collect_in_radius(position, definition.radius);
                }
            }
            TargetMode::PointBlank => self.collect_in_radius(origin, definition.radius),
            TargetMode::SelfCast | TargetMode::Movement => {}
        }
        self.hits.len()
    }

    /// Fills the hit list with every living hostile within `radius` of `centre`.
    pub(super) fn collect_in_radius(&mut self, centre: Vec3, radius: f32) {
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
            // Plus the target's own radius: a dummy whose centre is just outside the circle
            // but whose body is inside it should burn.
            if horizontal_distance(centre, position) <= radius + self.world.radius(id) {
                self.hits.push(id);
            }
        }
    }

    /// Fills the hit list with every living hostile inside a cone.
    fn collect_in_cone(&mut self, origin: Vec3, yaw: f32, range: f32) {
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
            if cosine >= CONE_HALF_ANGLE.cos() {
                self.hits.push(id);
            }
        }
    }

    /// Rolls a critical strike and applies every damage multiplier.
    fn roll_fire_damage(&mut self, base: f32) -> (f32, bool) {
        let crit = self.mage.roll_crit();
        let mut amount = base * self.mage.fire_damage_multiplier();
        if crit {
            amount *= crate::firemage::CRIT_MULTIPLIER;
        }
        self.mage.consume_spell_buffs();
        (amount, crit)
    }

    /// Deals fire damage and applies what a critical strike sets off.
    fn deal_fire_damage(
        &mut self,
        target: EntityId,
        amount: f32,
        ability: FireAbility,
        crit: bool,
    ) {
        self.deal_damage(
            self.player.entity,
            target,
            amount,
            AbilityId::from_fire(ability),
            DamageType::Fire,
        );

        // Inferno Cascade: a critical strike while the ring is up buys more of it.
        if crit {
            let effects = *self.mage.effects();
            if effects.inferno_crit_extension > 0.0
                && self.mage.zones.has(ZoneKind::Inferno)
                && self.mage.inferno_extension + effects.inferno_crit_extension
                    <= effects.inferno_max_extension
            {
                self.mage.inferno_extension += effects.inferno_crit_extension;
                self.mage
                    .zones
                    .extend(ZoneKind::Inferno, effects.inferno_crit_extension);
            }
        }
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

    /// Slows what stands in Cataclysm's fire.
    fn apply_slow(&mut self, target: EntityId) {
        if let Some(body) = self.world.body_mut(target) {
            body.velocity.x *= 1.0 - INFERNAL_GROUND_SLOW;
            body.velocity.z *= 1.0 - INFERNAL_GROUND_SLOW;
        }
    }

    /// Adds the Heat and Energy a cast generates.
    ///
    /// The cone pays per target up to its cap, which is why the count is passed in.
    fn gain_cast_resources(&mut self, ability: FireAbility, targets: u32) {
        let definition = ability.definition();
        let mut heat = definition.heat;
        if matches!(ability, FireAbility::FlameWave) {
            heat *= targets as f32;
        }
        if definition.heat_cap > 0.0 && !matches!(ability, FireAbility::Firestorm) {
            heat = heat.min(definition.heat_cap);
        }
        if matches!(ability, FireAbility::Firestorm) {
            // Firestorm's Heat arrives with its ticks, not with the cast.
            heat = 0.0;
        }
        self.mage.gain_heat(heat);
        self.mage.gain_energy(definition.energy);
    }

    /// Adds the Heat a zone tick generates, against that cast's cap.
    fn gain_zone_heat(&mut self, kind: ZoneKind, source: FireAbility) {
        let definition = source.definition();
        if definition.heat <= 0.0 {
            return;
        }
        let effects = *self.mage.effects();
        let scale = if kind == ZoneKind::Firestorm {
            effects.firestorm_heat_scale
        } else {
            1.0
        };
        let cap = definition.heat_cap * scale;
        let generated = definition.heat * scale;
        let spent = if kind == ZoneKind::Firestorm {
            &mut self.mage.firestorm_heat
        } else {
            &mut self.mage.zone_heat
        };
        let allowed = if cap > 0.0 {
            (cap - *spent).max(0.0).min(generated)
        } else {
            generated
        };
        if allowed <= 0.0 {
            return;
        }
        *spent += allowed;
        self.mage.gain_heat(allowed);
    }
}

/// Distance on the ground, ignoring height.
///
/// Every area in the class is a circle on the floor rather than a sphere: a dummy standing on
/// a step is inside the Firestorm it is standing in.
#[must_use]
pub fn horizontal_distance(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::arena::{self, ArenaConfig};
    use crate::firemage::{TalentId, ZoneKind};

    /// The training arena with the nearest dummy selected.
    fn arena() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation.select_nearest_target();
        simulation
    }

    fn target(simulation: &Simulation) -> EntityId {
        simulation.player().target.expect("a dummy is selected")
    }

    fn health(simulation: &Simulation, id: EntityId) -> f32 {
        simulation
            .world()
            .health(id)
            .map_or(0.0, |health| health.current())
    }

    /// Runs a cast to completion, however long it takes.
    fn cast_through(simulation: &mut Simulation, ability: FireAbility) {
        simulation.cast(ability).expect("the cast was refused");
        for _ in 0..240 {
            if simulation.casting().is_none() {
                break;
            }
            simulation.step_once();
        }
    }

    /// Puts `stacks` of Ignite on `id` without casting anything.
    fn ignite(simulation: &mut Simulation, id: EntityId, stacks: u8) {
        let maximum = simulation.mage().ignite_max();
        let duration = simulation.mage().ignite_duration();
        simulation
            .mage_mut()
            .ignite
            .apply(id, stacks, maximum, duration);
    }

    /// Puts the target at exactly the current maximum.
    fn ignite_to_maximum(simulation: &mut Simulation, id: EntityId) {
        let maximum = simulation.mage().ignite_max();
        ignite(simulation, id, maximum);
    }

    #[test]
    fn firebolt_always_ignites_an_unburnt_target() {
        // The designed asymmetry: guaranteed at zero stacks, a coin flip afterwards.
        for seed in 0..12u64 {
            let mut simulation = arena::training_arena(&ArenaConfig::default());
            simulation.step_once();
            simulation.select_nearest_target();
            simulation.mage_mut().rng = crate::firemage::Rng::new(seed + 1);

            let dummy = target(&simulation);
            assert_eq!(simulation.mage().ignite.stacks(dummy), 0);
            cast_through(&mut simulation, FireAbility::Firebolt);
            assert!(
                simulation.mage().ignite.stacks(dummy) >= 1,
                "seed {seed}: firebolt did not ignite a cold target"
            );
        }
    }

    #[test]
    fn firebolt_never_pushes_ignite_past_the_maximum() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        let maximum = simulation.mage().ignite_max();
        ignite(&mut simulation, dummy, maximum);

        for _ in 0..8 {
            cast_through(&mut simulation, FireAbility::Firebolt);
            assert_eq!(
                simulation.mage().ignite.stacks(dummy),
                maximum,
                "the cap leaked"
            );
        }
    }

    #[test]
    fn reaching_the_maximum_never_detonates_by_itself() {
        // The class's central rule. Stack to the cap, run for ten seconds, and the burn is
        // still sitting there waiting for the player.
        let mut simulation = arena();
        let dummy = target(&simulation);
        let maximum = simulation.mage().ignite_max();
        ignite(&mut simulation, dummy, maximum);

        simulation.step_many(60);
        assert_eq!(
            simulation.mage().ignite.stacks(dummy),
            maximum,
            "something consumed the burn"
        );
    }

    #[test]
    fn fire_detonation_is_refused_below_the_maximum_and_allowed_at_it() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        let maximum = simulation.mage().ignite_max();

        assert_eq!(
            simulation.cast(FireAbility::FireDetonation),
            Err(AbilityError::NotEnoughIgnite),
            "it fired on a cold target"
        );
        ignite(&mut simulation, dummy, maximum - 1);
        assert_eq!(
            simulation.cast(FireAbility::FireDetonation),
            Err(AbilityError::NotEnoughIgnite),
            "one stack short was enough"
        );

        ignite(&mut simulation, dummy, 1);
        assert!(simulation.cast(FireAbility::FireDetonation).is_ok());
        assert_eq!(
            simulation.mage().ignite.stacks(dummy),
            0,
            "the burn survived"
        );
    }

    #[test]
    fn a_refused_detonation_spends_no_charge() {
        let mut simulation = arena();
        let _ = simulation.cast(FireAbility::FireDetonation);
        assert!(
            simulation
                .mage()
                .abilities
                .is_ready(FireAbility::FireDetonation),
            "a refusal cost a charge"
        );
    }

    #[test]
    fn detonation_scales_with_the_stacks_it_consumes() {
        let mut small = arena();
        let dummy = target(&small);
        small.mage_mut().rng = crate::firemage::Rng::new(7);
        ignite_to_maximum(&mut small, dummy);
        let before = health(&small, dummy);
        small
            .cast(FireAbility::FireDetonation)
            .expect("a full burn");
        let full_burn = before - health(&small, dummy);

        // The same explosion with a talent that pays per stack must hit harder.
        let mut talented = arena();
        let dummy = target(&talented);
        talented.mage_mut().rng = crate::firemage::Rng::new(7);
        talented
            .mage_mut()
            .purchase_talent(TalentId::BurningEmbers)
            .expect("testing mode pays");
        ignite_to_maximum(&mut talented, dummy);
        let before = health(&talented, dummy);
        talented
            .cast(FireAbility::FireDetonation)
            .expect("a full burn");
        let with_talent = before - health(&talented, dummy);

        assert!(
            with_talent > full_burn,
            "burning embers did nothing: {with_talent} vs {full_burn}"
        );
    }

    #[test]
    fn detonation_splashes_and_ignites_what_is_nearby() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        ignite_to_maximum(&mut simulation, dummy);

        // Everything else in the arena that is within ten metres of the target.
        let neighbours: Vec<EntityId> = (0..simulation.world().slot_count())
            .filter_map(|slot| simulation.world().id_at(slot))
            .filter(|id| *id != dummy && *id != simulation.player().entity)
            .filter(|id| {
                let centre = simulation.world().position(dummy).unwrap_or(Vec3::ZERO);
                let other = simulation.world().position(*id).unwrap_or(Vec3::ZERO);
                horizontal_distance(centre, other) <= 10.0
            })
            .collect();
        let before: Vec<f32> = neighbours
            .iter()
            .map(|id| health(&simulation, *id))
            .collect();

        simulation
            .cast(FireAbility::FireDetonation)
            .expect("a full burn");

        for (id, was) in neighbours.iter().zip(before) {
            assert!(health(&simulation, *id) < was, "a neighbour took no splash");
            assert!(
                simulation.mage().ignite.stacks(*id) >= 1,
                "a neighbour was not ignited"
            );
        }
    }

    #[test]
    fn lingering_embers_leaves_a_stack_behind() {
        let mut simulation = arena();
        simulation
            .mage_mut()
            .purchase_talent(TalentId::LingeringEmbers)
            .expect("testing mode pays");
        let dummy = target(&simulation);
        ignite_to_maximum(&mut simulation, dummy);

        simulation
            .cast(FireAbility::FireDetonation)
            .expect("a full burn");
        assert_eq!(simulation.mage().ignite.stacks(dummy), 1);
    }

    #[test]
    fn infernal_capacity_moves_the_detonation_requirement_with_it() {
        // Section 18 of the brief, end to end: five stacks stop being enough.
        let mut simulation = arena();
        simulation
            .mage_mut()
            .purchase_talent(TalentId::InfernalCapacity)
            .expect("testing mode pays");
        assert_eq!(simulation.mage().ignite_max(), 6);

        let dummy = target(&simulation);
        ignite(&mut simulation, dummy, 5);
        assert_eq!(
            simulation.cast(FireAbility::FireDetonation),
            Err(AbilityError::NotEnoughIgnite),
            "five stacks still detonated with a six-stack cap"
        );

        ignite(&mut simulation, dummy, 1);
        assert_eq!(simulation.mage().ignite.stacks(dummy), 6);
        assert!(simulation.cast(FireAbility::FireDetonation).is_ok());
    }

    #[test]
    fn flame_lance_hits_harder_into_a_burn() {
        let mut cold = arena();
        cold.mage_mut().rng = crate::firemage::Rng::new(3);
        let dummy = target(&cold);
        let before = health(&cold, dummy);
        cast_through(&mut cold, FireAbility::FlameLance);
        let plain = before - health(&cold, dummy);

        let mut burning = arena();
        burning.mage_mut().rng = crate::firemage::Rng::new(3);
        let dummy = target(&burning);
        ignite(&mut burning, dummy, 1);
        let before = health(&burning, dummy);
        cast_through(&mut burning, FireAbility::FlameLance);
        let bonus = before - health(&burning, dummy);

        assert!(bonus > plain, "no ignite bonus: {bonus} vs {plain}");
    }

    #[test]
    fn pyroblast_always_crits_into_a_full_burn() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        ignite_to_maximum(&mut simulation, dummy);
        let before = health(&simulation, dummy);
        cast_through(&mut simulation, FireAbility::Pyroblast);
        let damage = before - health(&simulation, dummy);

        // Base 800, +10% per stack, doubled by the guaranteed critical strike.
        let base = FireAbility::Pyroblast.definition().damage;
        assert!(damage > base * 1.5, "it did not crit: {damage}");
        assert_eq!(
            simulation.mage().ignite.stacks(dummy),
            simulation.mage().ignite_max(),
            "pyroblast detonated the burn"
        );
    }

    #[test]
    fn flame_wave_ignites_everything_in_the_cone_and_caps_its_heat() {
        let mut simulation = arena();
        // Face the dummy group so the cone covers it.
        let target_id = target(&simulation);
        let origin = simulation.player_position();
        let towards = simulation.world().position(target_id).unwrap_or(Vec3::ZERO);
        let yaw = (-(towards.x - origin.x)).atan2(-(towards.z - origin.z));
        let player = simulation.player().entity;
        if let Some(body) = simulation.world_mut().body_mut(player) {
            body.facing_yaw = yaw;
        }

        cast_through(&mut simulation, FireAbility::FlameWave);
        assert!(
            simulation.mage().ignite.stacks(target_id) >= 1,
            "the cone did not ignite what it hit"
        );
        assert!(
            simulation.mage().heat.value() <= FireAbility::FlameWave.definition().heat_cap + 0.01,
            "the per-cast heat cap leaked: {}",
            simulation.mage().heat.value()
        );
    }

    #[test]
    fn meteor_lands_after_its_delay_rather_than_at_once() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        let position = simulation.world().position(dummy).expect("a position");
        let before = health(&simulation, dummy);

        simulation
            .cast_at(FireAbility::Meteor, position)
            .expect("in range");
        // Through the cast, then not yet through the impact delay.
        for _ in 0..60 {
            simulation.step_once();
        }
        assert_eq!(health(&simulation, dummy), before, "it landed early");

        simulation.step_many(120);
        assert!(health(&simulation, dummy) < before, "it never landed");
        assert!(
            simulation.mage().ignite.stacks(dummy) >= 2,
            "meteor applies two stacks"
        );
    }

    #[test]
    fn a_ground_ability_out_of_range_is_refused() {
        let mut simulation = arena();
        let far = Vec3::new(500.0, 0.0, 500.0);
        assert_eq!(
            simulation.cast_at(FireAbility::Meteor, far),
            Err(AbilityError::GroundOutOfRange)
        );
        assert!(simulation.mage().abilities.is_ready(FireAbility::Meteor));
    }

    #[test]
    fn a_ground_ability_cast_without_a_placement_says_so() {
        let mut simulation = arena();
        assert_eq!(
            simulation.cast(FireAbility::Firestorm),
            Err(AbilityError::NeedsGroundTarget)
        );
    }

    #[test]
    fn firestorm_leaves_a_zone_that_burns_and_then_expires() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        let position = simulation.world().position(dummy).expect("a position");
        let before = health(&simulation, dummy);

        simulation
            .cast_at(FireAbility::Firestorm, position)
            .expect("in range");
        // Through the 0.8 s cast and two of the zone's half-second ticks.
        simulation.step_many(120);
        assert!(simulation.mage().zones.has(ZoneKind::Firestorm), "no zone");
        assert!(health(&simulation, dummy) < before, "the zone did not burn");

        simulation.step_many(60 * 8);
        assert!(
            !simulation.mage().zones.has(ZoneKind::Firestorm),
            "the zone outlived its duration"
        );
    }

    #[test]
    fn inferno_follows_the_caster_and_ignites_what_it_touches() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        // Inferno is point blank: stand next to the dummy, which is what casting it means.
        let beside = simulation
            .world()
            .position(dummy)
            .map(|position| position + Vec3::new(2.0, 0.0, 0.0))
            .expect("a position");
        simulation.push_command(crate::command::PlayerCommand::Teleport { position: beside });
        simulation.step_once();
        simulation.cast(FireAbility::Inferno).expect("instant");
        assert!(simulation.mage().zones.has(ZoneKind::Inferno));

        simulation.step_many(70);
        assert!(
            simulation.mage().ignite.stacks(dummy) >= 1,
            "inferno did not ignite a nearby dummy"
        );
    }

    #[test]
    fn flame_shield_reduces_incoming_damage_while_it_lasts() {
        let mut simulation = arena();
        simulation.cast(FireAbility::FlameShield).expect("instant");
        assert!(simulation.mage().flame_shield.is_active());
        assert!((simulation.mage().incoming_damage(100.0) - 70.0).abs() < 0.01);

        simulation.step_many(60 * 6);
        assert!(!simulation.mage().flame_shield.is_active());
    }

    #[test]
    fn blink_moves_the_character_forward_and_goes_on_cooldown() {
        let mut simulation = arena();
        let before = simulation.player_position();
        simulation.cast(FireAbility::Blink).expect("instant");
        let after = simulation.player_position();

        let travelled = horizontal_distance(before, after);
        assert!(travelled > 1.0, "blink went nowhere: {travelled}");
        assert!(
            travelled <= FireAbility::Blink.definition().range + 0.01,
            "blink overshot: {travelled}"
        );
        assert!(!simulation.mage().abilities.is_ready(FireAbility::Blink));
    }

    #[test]
    fn casting_generates_heat_and_ultimate_energy() {
        let mut simulation = arena();
        assert_eq!(simulation.mage().heat.value(), 0.0);
        cast_through(&mut simulation, FireAbility::Firebolt);

        let firebolt = FireAbility::Firebolt.definition();
        assert!((simulation.mage().heat.value() - firebolt.heat).abs() < 0.01);
        assert!(simulation.mage().energy.value() >= firebolt.energy);
    }

    #[test]
    fn a_hundred_heat_overheats_and_then_drains() {
        let mut simulation = arena();
        simulation.mage_mut().gain_heat(100.0);
        assert!(simulation.mage().heat.is_overheated());
        assert!((simulation.mage().fire_damage_multiplier() - 1.15).abs() < 1e-4);

        simulation.step_many(60 * 10);
        assert!(!simulation.mage().heat.is_overheated(), "it never cooled");
    }

    #[test]
    fn the_ultimate_needs_a_full_bar_and_empties_it() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        let position = simulation.world().position(dummy).expect("a position");

        assert_eq!(
            simulation.cast_at(FireAbility::Cataclysm, position),
            Err(AbilityError::NotEnoughEnergy)
        );

        simulation.mage_mut().gain_energy(100.0);
        assert!(simulation.cast_at(FireAbility::Cataclysm, position).is_ok());
        assert!(
            simulation.mage().energy.value() < 100.0,
            "the bar was not spent"
        );
    }

    #[test]
    fn cataclysm_hits_hard_and_then_burns_the_ground() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        let position = simulation.world().position(dummy).expect("a position");
        let before = health(&simulation, dummy);

        simulation.mage_mut().gain_energy(100.0);
        simulation
            .cast_at(FireAbility::Cataclysm, position)
            .expect("a full bar");
        simulation.step_many(120);

        let after_impact = health(&simulation, dummy);
        assert!(after_impact < before, "the meteor never landed");
        assert!(
            simulation.mage().ignite.stacks(dummy) >= 3,
            "cataclysm applies three stacks"
        );
        assert!(
            simulation.mage().zones.has(ZoneKind::InfernalGround),
            "phase two never started"
        );

        simulation.step_many(120);
        assert!(
            health(&simulation, dummy) < after_impact,
            "the infernal ground did not burn"
        );
    }

    #[test]
    fn ignite_burns_over_time_and_falls_off_on_its_own() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        ignite(&mut simulation, dummy, 3);
        let before = health(&simulation, dummy);

        simulation.step_many(120);
        assert!(
            health(&simulation, dummy) < before,
            "the burn dealt nothing"
        );

        simulation.step_many(60 * 10);
        assert_eq!(
            simulation.mage().ignite.stacks(dummy),
            0,
            "it never expired"
        );
    }

    #[test]
    fn a_dead_target_stops_burning() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        ignite(&mut simulation, dummy, 5);
        simulation.world_mut().damage(dummy, 10_000.0);
        simulation.step_many(120);
        assert_eq!(simulation.mage().ignite.stacks(dummy), 0);
    }

    #[test]
    fn a_targeted_ability_is_refused_when_the_target_is_gone() {
        // The simulation retargets on its own, so "nothing selected" is a moment rather
        // than a state: what matters is that a dead target refuses cleanly and spends
        // nothing.
        let mut simulation = arena();
        let dummy = target(&simulation);
        simulation.world_mut().damage(dummy, 10_000.0);

        for ability in FireAbility::BAR {
            if !ability.definition().target.needs_target() {
                continue;
            }
            let refusal = simulation.cast(ability);
            assert!(
                matches!(
                    refusal,
                    Err(AbilityError::TargetInvalid
                        | AbilityError::NoTarget
                        | AbilityError::NotEnoughIgnite)
                ),
                "{ability:?} was allowed against a corpse: {refusal:?}"
            );
            assert!(
                simulation.mage().abilities.is_ready(ability),
                "{ability:?} spent a charge on a refusal"
            );
        }
    }

    #[test]
    fn an_out_of_range_target_is_refused() {
        let mut simulation = arena();
        let dummy = target(&simulation);
        if let Some(body) = simulation.world_mut().body_mut(dummy) {
            body.position = Vec3::new(0.0, 0.0, -500.0);
        }
        assert_eq!(
            simulation.cast(FireAbility::Firebolt),
            Err(AbilityError::OutOfRange)
        );
    }

    #[test]
    fn the_same_seed_produces_the_same_fight() {
        // Every chance in the class goes through the simulation's own generator, so a
        // replay of the same commands is the same fight.
        let run = || {
            let mut simulation = arena();
            for _ in 0..6 {
                let _ = simulation.cast(FireAbility::Firebolt);
                simulation.step_many(70);
            }
            simulation.meter().total_damage()
        };
        assert!((run() - run()).abs() < 1e-3);
    }
}
