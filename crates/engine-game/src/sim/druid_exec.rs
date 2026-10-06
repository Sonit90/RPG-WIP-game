//! Casting the Druid: Mana, Astral Power, and the thirteen abilities between them.
//!
//! ```text
//! a Druid cast
//!      │
//!      ├─ mana: paid from the one pool, announced as ManaSpent
//!      ├─ armed?  ──► spend 100 Astral Power ──► this cast is empowered
//!      ├─ the effect: damage through `strike`, healing through `heal_ally`,
//!      │              lasting effects through `Status`
//!      └─ astral: the row's own generation goes back into the bar
//! ```
//!
//! A child module of [`super`], exactly like [`super::firemage_exec`] and
//! [`super::warrior_exec`], and for the same reason: it needs the simulation's own state,
//! and the tick loop should not be buried in one class.
//!
//! # What this module does not own
//!
//! * **The numbers.** They are rows of [`crate::firemage::ABILITIES`] and named constants in
//!   [`crate::druid::tuning`].
//! * **Damage.** Every point of it goes through [`Simulation::strike`], which is the
//!   FireMage's function only in the sense that it lives in that file: it applies the
//!   caster's multipliers, rolls one critical strike and calls the one `deal_damage`.
//! * **Nature's Exposure's amplification and Astral Sunfire's conversion.** Both are read
//!   where damage is *applied*, so they answer for every source in the game rather than for
//!   the Druid's own spells — see [`Simulation::deal_damage`].
//! * **Ticking.** A heal over time is a [`Status`]; the status system ticks it and the
//!   simulation turns the outcome into a `Healing` event.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::druid::Druid;
use crate::druid::tuning::*;
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::firemage::{AbilityError, FireAbility, TargetMode, ZoneShape};
use crate::hero::Hero;
use crate::status::{Status, StatusEnd, StatusKind};

use super::Simulation;

/// The Druid's heals over time, in the order Swiftmend eats them.
///
/// Longest-lived first, so consuming one leaves the target with the effect that had least
/// left to give. One list, read by Swiftmend and by nothing else — what *counts* as a Druid
/// heal over time is [`StatusKind::is_druid_hot`], which Nourish asks instead.
const SWIFTMEND_ORDER: [StatusKind; 4] = [
    StatusKind::Rejuvenation,
    StatusKind::Regrowth,
    StatusKind::WildGrowth,
    StatusKind::Lifebloom,
];

impl Simulation {
    /// The Druid's state: Mana, Astral Power and the channel.
    #[must_use]
    pub fn druid(&self) -> &crate::druid::Druid {
        &self.druid
    }

    /// The Druid's state, for a developer control or a test setting up a situation.
    pub fn druid_mut(&mut self) -> &mut crate::druid::Druid {
        &mut self.druid
    }

    /// Whether the player is the Druid. Every rule in this file is off for everybody else.
    #[must_use]
    pub(super) fn is_druid(&self) -> bool {
        self.hero == Hero::Druid
    }

    // --------------------------------------------------------------- validation

    /// **What refuses a Druid cast**, on top of the rules every ability already obeys.
    ///
    /// Asked by `try_cast` before anything is spent, so a refusal costs neither the charge
    /// nor the global cooldown.
    ///
    /// # Errors
    ///
    /// Returns the reason the cast cannot happen.
    pub(super) fn druid_can_cast(&self, ability: FireAbility) -> Result<(), AbilityError> {
        if !self.is_druid() {
            return Ok(());
        }
        let druid_row = ability.definition().druid();
        if druid_row.mana_cost > 0.0 && !self.druid.mana.can_pay(druid_row.mana_cost) {
            return Err(AbilityError::NotEnoughMana);
        }
        match ability {
            // The button. A full bar, and not already armed: pressing it twice would spend
            // nothing and say nothing.
            FireAbility::AstralPower if !self.druid.astral.can_arm() => {
                Err(AbilityError::NotEnoughAstralPower)
            }
            // Swiftmend has something to eat, or it is refused before it costs anything.
            FireAbility::Swiftmend => {
                let target = self
                    .friendly_target()
                    .ok_or(AbilityError::NoFriendlyTarget)?;
                if self.swiftmend_victim(target).is_some() {
                    Ok(())
                } else {
                    Err(AbilityError::NoHealOverTime)
                }
            }
            _ => Ok(()),
        }
    }

    /// **Who a friendly ability is pointed at.**
    ///
    /// The selection when it is friendly and alive, and the caster when it is not. One
    /// function, asked by the validation, by the hit list and by the interface, so a heal
    /// cannot be aimed at one ally and land on another.
    #[must_use]
    pub fn friendly_target(&self) -> Option<EntityId> {
        let caster = self.player.entity;
        if let Some(target) = self.player.target
            && target != caster
            && self.world.is_alive(target)
            && !self.is_attackable(target)
        {
            return Some(target);
        }
        self.world.is_alive(caster).then_some(caster)
    }

    /// The heal over time Swiftmend would consume from `target`, if there is one.
    #[must_use]
    pub fn swiftmend_victim(&self, target: EntityId) -> Option<StatusKind> {
        let statuses = self.world.statuses(target)?;
        SWIFTMEND_ORDER.into_iter().find(|kind| statuses.has(*kind))
    }

    /// How many of the Druid's heals over time are on `target`.
    ///
    /// What Nourish is scaled by, and deliberately uncapped: six of them is sixty per cent.
    #[must_use]
    pub fn druid_hot_count(&self, target: EntityId) -> u32 {
        self.world.statuses(target).map_or(0, |statuses| {
            statuses
                .iter()
                .filter(|status| status.kind.is_druid_hot())
                .count() as u32
        })
    }

    // --------------------------------------------------------------- resources

    /// Pays this ability's Mana, if it costs any, and announces it.
    pub(super) fn spend_mana(&mut self, ability: FireAbility) {
        let cost = ability.definition().druid().mana_cost;
        if !self.is_druid() || cost <= 0.0 || !self.druid.mana.pay(cost) {
            return;
        }
        let value = self.druid.mana.current();
        let tick = self.tick;
        self.events.push(CombatEvent::ManaSpent {
            entity: self.player.entity,
            amount: cost,
            value,
            ability: AbilityId::from_fire(ability),
            tick,
        });
    }

    /// Gives this ability's Astral Power back to the bar, and announces it.
    pub(super) fn gain_astral(&mut self, ability: FireAbility) {
        let generated = ability.definition().druid().astral_generation;
        if !self.is_druid() || generated <= 0.0 {
            return;
        }
        let applied = self.druid.astral.gain(generated);
        if applied <= 0.0 {
            return;
        }
        let value = self.druid.astral.value();
        let tick = self.tick;
        self.events.push(CombatEvent::AstralPowerGained {
            entity: self.player.entity,
            amount: applied,
            value,
            ability: AbilityId::from_fire(ability),
            tick,
        });
    }

    /// **Spends the arming on this ability, if it is compatible and there is one.**
    ///
    /// Returns whether the cast is empowered. The one place Astral Power goes down, and the
    /// one place the compatibility list is consulted — which is the ability's own
    /// [`crate::firemage::DruidRow::astral_empowerable`] column, so Nature's Cleanse, Wild
    /// Rush and the button itself are excluded by data rather than by a branch.
    pub(super) fn take_empowerment(&mut self, ability: FireAbility) -> bool {
        if !self.is_druid() || !ability.definition().druid().astral_empowerable {
            return false;
        }
        if !self.druid.astral.is_armed() || !self.druid.astral.consume() {
            return false;
        }
        let value = self.druid.astral.value();
        let tick = self.tick;
        self.events.push(CombatEvent::AstralPowerSpent {
            entity: self.player.entity,
            amount: ASTRAL_SPEND,
            value,
            ability: AbilityId::from_fire(ability),
            tick,
        });
        true
    }

    /// Whether the next compatible cast will be empowered. What the bar highlights.
    #[must_use]
    pub fn astral_armed(&self) -> bool {
        self.is_druid() && self.druid.astral.is_armed()
    }

    // --------------------------------------------------------------- healing

    /// **Heals `target` and announces it.** Returns how much landed.
    ///
    /// The one place a Druid heal reaches the world. It goes through `GameWorld::heal` and
    /// the `Healing` event exactly as the Warrior's self heal does, so a heal on an ally and
    /// a heal on yourself are the same event to the meter, the log and the interface.
    pub(super) fn heal_ally(&mut self, target: EntityId, amount: f32, ability: AbilityId) -> f32 {
        if self.remote_authority
            || !self.world.is_alive(target)
            || !amount.is_finite()
            || amount <= 0.0
        {
            return 0.0;
        }
        let restored = self.world.heal(target, amount).applied;
        if restored <= 0.0 {
            return 0.0;
        }
        let tick = self.tick;
        self.events.push(CombatEvent::Healing {
            source: self.player.entity,
            target,
            amount: restored,
            ability,
            tick,
        });
        self.add_healing_threat(self.player.entity, target, restored, ability);
        restored
    }

    // --------------------------------------------------------------- the kit

    /// **Applies a Druid ability whose targets have already been collected.**
    ///
    /// Reached from [`super::Simulation::land`], which has already spent the charge and
    /// emitted `AbilityUsed`. Mana and Astral Power are settled here, in one place, so no
    /// ability can be cast for free by forgetting a line.
    pub(super) fn land_druid(&mut self, ability: FireAbility, hits: usize) -> u32 {
        self.spend_mana(ability);
        let affected = match ability {
            FireAbility::Moonfire => self.cast_moonfire(),
            FireAbility::Sunfire => self.cast_sunfire(),
            FireAbility::Wrath => self.cast_wrath(),
            FireAbility::Rejuvenation => self.cast_rejuvenation(),
            FireAbility::Regrowth => self.cast_regrowth(),
            FireAbility::Lifebloom => self.cast_lifebloom(),
            FireAbility::WildGrowth => self.cast_wild_growth(),
            FireAbility::Swiftmend => self.cast_swiftmend(),
            FireAbility::Nourish => self.cast_nourish(),
            FireAbility::NaturesCleanse => self.cast_natures_cleanse(),
            FireAbility::WildRush => self.cast_wild_rush(),
            FireAbility::AstralPower => self.cast_astral_power(),
            // The channel's healing happens while it runs, not when it ends. Everything it
            // owes — the empowerment, the pulse clock — was settled when it started.
            FireAbility::Tranquility => {
                self.druid.channel.active = false;
                0
            }
            _ => hits as u32,
        };
        self.gain_astral(ability);
        affected
    }

    /// Moonfire: moonlight now, and eighteen seconds of it afterwards.
    ///
    /// Empowered, the two halves are raised independently — the impact by its own constant
    /// and the burn by the same one — and the burn stops expiring altogether.
    fn cast_moonfire(&mut self) -> u32 {
        let ability = FireAbility::Moonfire;
        let definition = *ability.definition();
        let empowered = self.take_empowerment(ability);
        let scale = if empowered {
            1.0 + ASTRAL_MOONFIRE_DAMAGE
        } else {
            1.0
        };
        let stats = self.mage.stats();
        let source = self.player.entity;
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            self.strike(
                target,
                definition.base_damage(&stats) * scale,
                ability,
                true,
                0.0,
            );
            if !self.world.is_alive(target) {
                continue;
            }
            let mut status = Status::new(
                StatusKind::Moonfire,
                source,
                AbilityId::from_fire(ability),
                definition.duration,
                definition.base_tick_damage(&stats) * scale,
            )
            .every(definition.tick_interval)
            .with_haste()
            .empowered_by_astral(empowered);
            if empowered {
                // The whole of "it never ends". A death, an encounter reset or a dispel take
                // it; time does not.
                status = status.forever();
            }
            self.apply_status(target, status, definition.duration);
            affected += 1;
        }
        affected
    }

    /// Sunfire: the same burn, on everything standing near the target.
    ///
    /// It collects its own hit list rather than going through the shared single-target one,
    /// because "the target and the pack around it" is a shape no other ability in the game
    /// asks for.
    fn cast_sunfire(&mut self) -> u32 {
        let ability = FireAbility::Sunfire;
        let definition = *ability.definition();
        let empowered = self.take_empowerment(ability);
        let scale = if empowered {
            1.0 + ASTRAL_SUNFIRE_DAMAGE
        } else {
            1.0
        };
        let Some(primary) = self.player.target.filter(|id| self.world.is_alive(*id)) else {
            return 0;
        };
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

        let stats = self.mage.stats();
        let source = self.player.entity;
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            self.strike(
                target,
                definition.base_damage(&stats) * scale,
                ability,
                true,
                0.0,
            );
            if !self.world.is_alive(target) {
                continue;
            }
            let status = Status::new(
                StatusKind::Sunfire,
                source,
                AbilityId::from_fire(ability),
                definition.duration,
                definition.base_tick_damage(&stats) * scale,
            )
            .every(definition.tick_interval)
            .with_haste()
            .empowered_by_astral(empowered);
            self.apply_status(target, status, definition.duration);
            if empowered {
                // The mark, which deals nothing. What it does happens where damage is
                // applied, to every party member's damage rather than to the Druid's.
                let conversion = Status::new(
                    StatusKind::AstralSunfire,
                    source,
                    AbilityId::from_fire(ability),
                    ASTRAL_SUNFIRE_DURATION,
                    0.0,
                );
                self.apply_status(target, conversion, ASTRAL_SUNFIRE_DURATION);
            }
            affected += 1;
        }
        affected
    }

    /// Wrath: free, and the debuff every cast of it stacks.
    fn cast_wrath(&mut self) -> u32 {
        let ability = FireAbility::Wrath;
        let definition = *ability.definition();
        let empowered = self.take_empowerment(ability);
        let stats = self.mage.stats();
        let source = self.player.entity;

        if empowered {
            // The buff is applied first, so the very cast that paid for it already goes wide.
            let buff = Status::new(
                StatusKind::AstralWrath,
                source,
                AbilityId::from_fire(ability),
                ASTRAL_WRATH_DURATION,
                0.0,
            )
            .empowered_by_astral(true);
            self.apply_status(source, buff, ASTRAL_WRATH_DURATION);
        }

        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            self.strike(target, definition.base_damage(&stats), ability, true, 0.0);
            if !self.world.is_alive(target) {
                continue;
            }
            self.apply_natures_exposure(target);
            affected += 1;
        }
        if affected == 0 {
            return 0;
        }

        // Astral Wrath: everything within ten metres of the primary target takes the debuff
        // as well. The primary is excluded, so one cast is one stack on it.
        if self
            .world
            .statuses(source)
            .is_some_and(|statuses| statuses.has(StatusKind::AstralWrath))
        {
            self.spread_natures_exposure();
        }

        // A share of maximum Mana, on a roll, through the class's own random source so a
        // replay of the same fight restores it on the same casts.
        if self.mage.rng.chance(WRATH_MANA_CHANCE) {
            let amount = self.druid.mana.max() * WRATH_MANA_RESTORE;
            let restored = self.druid.mana.gain(amount);
            if restored > 0.0 {
                let value = self.druid.mana.current();
                let tick = self.tick;
                self.events.push(CombatEvent::ManaRestored {
                    entity: source,
                    amount: restored,
                    value,
                    ability: AbilityId::from_fire(ability),
                    tick,
                });
            }
        }
        affected
    }

    /// Puts one stack of Nature's Exposure on `target`, refreshing what is there.
    ///
    /// The stack ceiling is the status kind's own, so nothing here can overshoot it.
    fn apply_natures_exposure(&mut self, target: EntityId) {
        let status = Status::new(
            StatusKind::NaturesExposure,
            self.player.entity,
            AbilityId::from_fire(FireAbility::Wrath),
            NATURES_EXPOSURE_DURATION,
            0.0,
        );
        self.apply_status(target, status, NATURES_EXPOSURE_DURATION);
    }

    /// Astral Wrath's area application: the same stack, on everything around the primary.
    fn spread_natures_exposure(&mut self) {
        let Some(primary) = self.player.target else {
            return;
        };
        let Some(centre) = self.world.position(primary) else {
            return;
        };
        // The existing area query, so "within ten metres" means what it means everywhere
        // else — edge to edge, living hostiles only.
        self.hits.clear();
        self.collect_in_shape(
            centre,
            ZoneShape::Circle {
                radius: ASTRAL_WRATH_RADIUS,
            },
        );
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            if target == primary {
                continue;
            }
            self.apply_natures_exposure(target);
        }
    }

    /// Rejuvenation: eighteen seconds of healing and nothing else.
    fn cast_rejuvenation(&mut self) -> u32 {
        self.apply_druid_hot(FireAbility::Rejuvenation, StatusKind::Rejuvenation, 0.0)
    }

    /// Regrowth: a large heal now, and the rest of it over twelve seconds.
    fn cast_regrowth(&mut self) -> u32 {
        let ability = FireAbility::Regrowth;
        let empowered = self.take_empowerment(ability);
        let stats = self.mage.stats();
        let definition = *ability.definition();
        let scale = Druid::heal_scale(empowered);
        let Some(target) = self.hits.first().copied() else {
            return 0;
        };
        self.heal_ally(
            target,
            definition.base_damage(&stats) * scale,
            AbilityId::from_fire(ability),
        );
        self.apply_hot_to(target, ability, StatusKind::Regrowth, empowered, 0.0);
        1
    }

    /// Lifebloom: fifteen seconds of healing, and a bloom when it runs out.
    fn cast_lifebloom(&mut self) -> u32 {
        let ability = FireAbility::Lifebloom;
        let bloom = ability.definition().druid().bloom_coefficient * self.mage.stats().spell_power;
        self.apply_druid_hot(ability, StatusKind::Lifebloom, bloom)
    }

    /// Wild Growth: the target, and the hurt allies standing near them.
    fn cast_wild_growth(&mut self) -> u32 {
        let ability = FireAbility::WildGrowth;
        let empowered = self.take_empowerment(ability);
        let Some(primary) = self.hits.first().copied() else {
            return 0;
        };
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            // The primary always; everybody else only if they have something to heal, which
            // is what "nearby injured allies" means.
            let injured = self
                .world
                .health(target)
                .is_some_and(|health| health.current() < health.max());
            if target != primary && !injured {
                continue;
            }
            self.apply_hot_to(target, ability, StatusKind::WildGrowth, empowered, 0.0);
            affected += 1;
        }
        affected
    }

    /// Swiftmend: eat one heal over time, and pay it out at once.
    fn cast_swiftmend(&mut self) -> u32 {
        let ability = FireAbility::Swiftmend;
        let empowered = self.take_empowerment(ability);
        let Some(target) = self.hits.first().copied() else {
            return 0;
        };
        let Some(victim) = self.swiftmend_victim(target) else {
            return 0;
        };
        // Consumed, not expired: a Lifebloom eaten by Swiftmend does not also bloom.
        if let Some(statuses) = self.world.statuses_mut(target) {
            statuses.consume(victim);
        }
        let tick = self.tick;
        self.events.push(CombatEvent::StatusRemoved {
            target,
            status: victim,
            reason: StatusEnd::Consumed,
            tick,
        });
        let stats = self.mage.stats();
        let amount = ability.definition().base_damage(&stats) * Druid::heal_scale(empowered);
        self.heal_ally(target, amount, AbilityId::from_fire(ability));
        1
    }

    /// Nourish: larger the more of the Druid's work is already on the target.
    fn cast_nourish(&mut self) -> u32 {
        let ability = FireAbility::Nourish;
        let empowered = self.take_empowerment(ability);
        let Some(target) = self.hits.first().copied() else {
            return 0;
        };
        let stats = self.mage.stats();
        // The heal-over-time scaling first, and the empowerment on top of the result: the
        // design states the thirty per cent applies *after* the count.
        let hots = self.druid_hot_count(target);
        let scaling = 1.0 + hots as f32 * NOURISH_PER_HOT;
        let amount =
            ability.definition().base_damage(&stats) * scaling * Druid::heal_scale(empowered);
        self.heal_ally(target, amount, AbilityId::from_fire(ability));
        1
    }

    /// Nature's Cleanse: everything harmful a school can name comes off.
    ///
    /// Not empowerable, by its row rather than by a branch here.
    fn cast_natures_cleanse(&mut self) -> u32 {
        let ability = FireAbility::NaturesCleanse;
        let Some(target) = self.hits.first().copied() else {
            return 0;
        };
        let mut removed: Vec<(StatusKind, crate::status::DispelSchool)> = Vec::new();
        if let Some(statuses) = self.world.statuses(target) {
            for status in statuses.iter() {
                if let Some(school) = status.kind.dispel_school()
                    && status.kind.is_harmful()
                {
                    removed.push((status.kind, school));
                }
            }
        }
        if removed.is_empty() {
            return 0;
        }
        let source = self.player.entity;
        let tick = self.tick;
        for (kind, school) in &removed {
            if let Some(statuses) = self.world.statuses_mut(target) {
                statuses.consume(*kind);
            }
            self.events.push(CombatEvent::Dispelled {
                source,
                target,
                status: *kind,
                school: *school,
                ability: AbilityId::from_fire(ability),
                tick,
            });
            self.events.push(CombatEvent::StatusRemoved {
                target,
                status: *kind,
                reason: StatusEnd::Consumed,
                tick,
            });
        }
        removed.len() as u32
    }

    /// Wild Rush: six seconds of being faster and harder to hurt.
    ///
    /// A status rather than a timer on the class, so it appears on the player's frame, is
    /// removed by death and by an encounter reset with everything else, and has exactly one
    /// truth about whether it is running.
    fn cast_wild_rush(&mut self) -> u32 {
        let ability = FireAbility::WildRush;
        let definition = *ability.definition();
        let source = self.player.entity;
        let status = Status::new(
            StatusKind::WildRush,
            source,
            AbilityId::from_fire(ability),
            definition.duration,
            0.0,
        );
        self.apply_status(source, status, definition.duration);
        1
    }

    /// The Astral Power button: arms the next compatible spell and spends nothing.
    fn cast_astral_power(&mut self) -> u32 {
        if !self.druid.astral.arm() {
            return 0;
        }
        let value = self.druid.astral.value();
        let tick = self.tick;
        self.events.push(CombatEvent::AstralPowerArmed {
            entity: self.player.entity,
            value,
            tick,
        });
        1
    }

    /// Applies one of the Druid's heals over time to the first friendly target.
    fn apply_druid_hot(&mut self, ability: FireAbility, kind: StatusKind, bloom: f32) -> u32 {
        let empowered = self.take_empowerment(ability);
        let Some(target) = self.hits.first().copied() else {
            return 0;
        };
        self.apply_hot_to(target, ability, kind, empowered, bloom);
        1
    }

    /// Applies one heal over time, with the empowerment already multiplied into every number
    /// it will ever pay out.
    ///
    /// **The one place a Druid heal over time is built.** The thirty per cent is applied to
    /// the tick and to the bloom here and nowhere else, which is what makes "every healing
    /// event, exactly once" structural.
    fn apply_hot_to(
        &mut self,
        target: EntityId,
        ability: FireAbility,
        kind: StatusKind,
        empowered: bool,
        bloom: f32,
    ) {
        let definition = *ability.definition();
        let stats = self.mage.stats();
        let scale = Druid::heal_scale(empowered);
        let status = Status::new(
            kind,
            self.player.entity,
            AbilityId::from_fire(ability),
            definition.duration,
            definition.base_tick_damage(&stats) * scale,
        )
        .every(definition.tick_interval)
        .with_haste()
        .empowered_by_astral(empowered)
        .blooming(bloom * scale);
        self.apply_status(target, status, definition.duration);
    }

    // --------------------------------------------------------------- the channel

    /// Starts Tranquility's channel, if that is what was just cast.
    ///
    /// **The empowerment is taken here**, when the channel begins, rather than when it ends:
    /// its pulses are empowered from the first one, and a decision that was made after they
    /// had already been paid out would be a decision that could not apply to them.
    pub(super) fn begin_druid_channel(&mut self, ability: FireAbility) {
        if !self.is_druid() || ability != FireAbility::Tranquility {
            return;
        }
        let empowered = self.take_empowerment(ability);
        self.druid.channel = crate::druid::Channel {
            pulses: 0,
            empowered,
            active: true,
        };
    }

    /// Advances everything the Druid owns, and pulses a channel that is running.
    ///
    /// **The pulses are scheduled off the cast's own elapsed time**, not off a clock of their
    /// own: the *n*th pulse happens once the channel has been running for `n × interval`
    /// seconds, and the number of them is the duration divided by the interval. A six-second
    /// channel therefore pays out six pulses on any machine, at any frame rate, however the
    /// fixed step rounds.
    pub(super) fn tick_druid(&mut self, step: f32) {
        self.druid.tick(step);
        if !self.druid.channel.active {
            return;
        }
        // The cast bar *is* the channel. If it stopped — moved, cancelled, interrupted, the
        // caster died — the pulses stop with it, and nothing here has to know which of those
        // happened.
        let elapsed = self
            .player
            .cast
            .filter(|cast| cast.ability == FireAbility::Tranquility)
            .map(|cast| cast.elapsed);
        let Some(elapsed) = elapsed else {
            self.druid.channel.active = false;
            return;
        };

        let definition = FireAbility::Tranquility.definition();
        let interval = definition.tick_interval.max(crate::TIME_EPSILON);
        // The channel's length is its cast time: a channel is a cast that does something
        // while it runs, which is why moving out of one already stops it.
        let total = (definition.cast_time / interval).round().max(0.0) as u32;
        while self.druid.channel.pulses < total
            && elapsed + crate::TIME_EPSILON >= self.druid.channel.pulses as f32 * interval
        {
            self.druid.channel.pulses += 1;
            self.pulse_tranquility();
        }
    }

    /// One healing pulse of Tranquility: every ally inside the radius, at once.
    fn pulse_tranquility(&mut self) {
        let ability = FireAbility::Tranquility;
        let definition = *ability.definition();
        let stats = self.mage.stats();
        let amount =
            definition.base_tick_damage(&stats) * Druid::heal_scale(self.druid.channel.empowered);
        let origin = self
            .world
            .position(self.player.entity)
            .unwrap_or(Vec3::ZERO);
        self.hits.clear();
        self.collect_friendly_in_shape(
            origin,
            ZoneShape::Circle {
                radius: definition.radius,
            },
        );
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            self.heal_ally(target, amount, AbilityId::from_fire(ability));
        }
    }

    // --------------------------------------------------------------- targeting

    /// Fills the hit list with every living friendly inside `shape` anchored at `anchor`.
    ///
    /// The counterpart of [`Simulation::collect_in_shape`], which answers the same question
    /// about enemies. Two functions rather than one with a flag, because "who may I hit" and
    /// "who may I heal" are different questions and a flag would make every call site read
    /// like the wrong one.
    pub(super) fn collect_friendly_in_shape(&mut self, anchor: Vec3, shape: ZoneShape) {
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if !self.world.is_alive(id) || self.is_attackable(id) {
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

    /// Fills the hit list for a friendly ability: the target, and its radius if it has one.
    pub(super) fn collect_friendly_targets(&mut self, ability: FireAbility) {
        let definition = ability.definition();
        self.hits.clear();
        let Some(primary) = self.friendly_target() else {
            return;
        };
        if definition.radius <= 0.0 {
            self.hits.push(primary);
            return;
        }
        let Some(centre) = self.world.position(primary) else {
            self.hits.push(primary);
            return;
        };
        self.collect_friendly_in_shape(
            centre,
            ZoneShape::Circle {
                radius: definition.radius,
            },
        );
        // The primary is healed once and is healed first: it is added only if the area query
        // did not already find it, and swapped to the front either way, so every ability that
        // treats "the target" differently from "the others" can simply read the first entry.
        match self.hits.iter().position(|id| *id == primary) {
            Some(index) => self.hits.swap(0, index),
            None => {
                self.hits.push(primary);
                let last = self.hits.len() - 1;
                self.hits.swap(0, last);
            }
        }
    }

    // --------------------------------------------------------------- passives

    /// Whether the player is under Wild Rush right now.
    #[must_use]
    pub fn wild_rush_active(&self) -> bool {
        self.is_druid()
            && self
                .world
                .statuses(self.player.entity)
                .is_some_and(|statuses| statuses.has(StatusKind::WildRush))
    }

    /// What the player's movement speed is multiplied by. One when nothing is multiplying it.
    ///
    /// The effects on the player, read from the registry: Wild Rush's burst, a crippling
    /// shot's slow, anything else that names movement speed — the same evaluation a monster's
    /// speed goes through.
    #[must_use]
    pub fn move_speed_multiplier(&self) -> f32 {
        self.world
            .statuses(self.player.entity)
            .map_or(1.0, crate::status::StatusSet::move_speed_multiplier)
    }

    /// The flat damage reduction `entity`'s effects give it, as a fraction: Wild Rush's.
    #[must_use]
    pub(super) fn status_damage_reduction(&self, entity: EntityId) -> f32 {
        self.world
            .statuses(entity)
            .map_or(0.0, crate::status::StatusSet::damage_reduction)
    }

    /// **What the effects on `target` multiply an incoming hit by**: Nature's Exposure, Magic
    /// Vulnerability, anything in the registry that names damage taken.
    ///
    /// Read where damage is applied, so it answers for every source of damage in the game
    /// rather than for one class's spells.
    #[must_use]
    pub(super) fn damage_taken_multiplier(
        &self,
        target: EntityId,
        damage_type: crate::combat::DamageType,
    ) -> f32 {
        self.world.statuses(target).map_or(1.0, |statuses| {
            statuses.damage_taken_multiplier(damage_type)
        })
    }

    /// What the effects on `source` multiply the damage it deals by: Bloodlust, a curse.
    #[must_use]
    pub(super) fn damage_dealt_multiplier(&self, source: EntityId) -> f32 {
        self.world
            .statuses(source)
            .map_or(1.0, crate::status::StatusSet::damage_dealt_multiplier)
    }

    /// **Astral Sunfire's conversion**: a share of what the party dealt becomes healing.
    ///
    /// Called once, from the one place damage is applied, with the damage that *landed*.
    ///
    /// Three things keep it from feeding itself: healing is not damage and cannot re-enter
    /// this path, the mark is only ever read for damage dealt **to** its bearer, and the
    /// re-entrancy guard refuses a second conversion inside the first. The last is belt and
    /// braces for a future effect that turns healing into damage.
    pub(super) fn convert_astral_sunfire(
        &mut self,
        source: EntityId,
        target: EntityId,
        dealt: f32,
    ) {
        if self.converting || dealt <= 0.0 {
            return;
        }
        let marked = self
            .world
            .statuses(target)
            .is_some_and(|statuses| statuses.has(StatusKind::AstralSunfire));
        if !marked {
            return;
        }
        // Only what the party dealt. An enemy that damages another enemy standing in a
        // Sunfire heals nobody.
        if !self.party.contains(source) {
            return;
        }
        let amount = dealt * ASTRAL_SUNFIRE_CONVERSION;
        if amount <= 0.0 {
            return;
        }
        self.converting = true;
        let members: Vec<EntityId> = self.party.members().to_vec();
        for member in members {
            self.heal_ally(member, amount, AbilityId::from_fire(FireAbility::Sunfire));
        }
        self.converting = false;
    }

    /// Pays out a Lifebloom that ran out of time on its own.
    pub(super) fn bloom(&mut self, target: EntityId, ability: AbilityId, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        self.heal_ally(target, amount, ability);
    }
}

/// Whether an ability is aimed at an ally.
///
/// A free function so the hit list, the validation and the interface can ask without a
/// simulation in hand.
#[must_use]
pub fn is_friendly_ability(ability: FireAbility) -> bool {
    matches!(ability.definition().target, TargetMode::Friendly)
}
