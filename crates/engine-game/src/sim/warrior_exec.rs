//! Casting the Warrior: Deflection in, Deflection out, and the eleven abilities between.
//!
//! ```text
//! an attack arrives
//!      │
//!      ├─ Spell Reflect?  ──► back to the caster, and nothing lands
//!      │
//!      ├─ DefenseProfile::roll  (one roll, one outcome)
//!      │        └─ defensive? ──┬─ +0.2 (or +0.4) Deflection
//!      │                        ├─ Battle Trance: 1 % max HP, three times a second
//!      │                        └─ Spell Reflect: −0.1 s off its cooldown
//!      ├─ mitigate: block share, armour, flat reduction
//!      ├─ absorb: Puncture Blood's shield stands in front of health
//!      └─ Adrenaline banks its share of what actually landed
//! ```
//!
//! A child module of [`super`], exactly like [`super::firemage_exec`], and for the same
//! reason: it needs the simulation's own state, and the tick loop should not be buried in
//! one class.
//!
//! # What this module does not own
//!
//! * **The abilities' numbers.** They are rows of [`crate::firemage::ABILITIES`] and named
//!   constants in [`crate::warrior::tuning`]. Nothing here writes a number down.
//! * **The roll.** [`crate::character::DefenseProfile`] decides Block, Parry, Dodge and
//!   Armour, once per attack, for anybody who has the stats.
//! * **Threat.** It is damage, in [`super::Simulation::deal_damage`], with a multiplier off
//!   the ability's own row.
//! * **Stun and silence.** They are properties of the enemy's mind.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::character::stat::{Stat, health_from_stamina};
use crate::character::{DefenseOutcome, DefenseProfile};
use crate::combat::DamageType;
use crate::enemy::EnemyEffect;
use crate::entity::EntityId;
use crate::event::{CombatEvent, ControlKind};
use crate::firemage::{FireAbility, ZoneShape};
use crate::hero::Hero;
use crate::status::{Status, StatusKind};
use crate::warrior::tuning::*;
use crate::warrior::{DEFLECTION_SPEND, health_scaled};

use super::Simulation;

impl Simulation {
    /// The Warrior's state: the resource, the timers, the absorb and the banked pool.
    #[must_use]
    pub fn warrior(&self) -> &crate::warrior::Warrior {
        &self.warrior
    }

    /// The Warrior's state, for a developer control or a test setting up a situation.
    pub fn warrior_mut(&mut self) -> &mut crate::warrior::Warrior {
        &mut self.warrior
    }

    /// **What an attack on the player meets right now.**
    ///
    /// Derived on every call and never stored: the Warrior's armour changes on the tick they
    /// spend Deflection, and their Block changes on the tick Shield Block starts. A stored
    /// profile would be a snapshot of the one thing in the class that must not be one.
    #[must_use]
    pub fn defense_profile(&self) -> DefenseProfile {
        let stats = self.character_stats();
        let mut profile = DefenseProfile::from_stats(&stats);
        if self.hero != Hero::Warrior {
            // Every other hero's own reduction, through the same field the Warrior's Last
            // Stand uses: one number, applied once, where damage is mitigated.
            profile.damage_reduction = self.druid_damage_reduction();
            return profile;
        }
        profile.armor *= self.warrior.armor_multiplier();
        profile.block += self.warrior.shield_block_bonus(self.scaling_health());
        profile.immune_to_critical = self.warrior.immune_to_critical();
        profile.damage_reduction = self.warrior.damage_reduction();
        profile
    }

    /// The player's maximum health, including everything currently multiplying it.
    #[must_use]
    pub fn player_max_health(&self) -> f32 {
        self.world
            .health(self.player.entity)
            .map_or(0.0, crate::combat::Health::max)
    }

    /// **The maximum health the health-scaled abilities are measured against.**
    ///
    /// Stamina and gear, *without* the class's own multipliers. Shield Block and Last Stand
    /// both scale from this rather than from [`Simulation::player_max_health`], and both for
    /// the same reason: the live maximum already contains the Deflection bonus and Last
    /// Stand's own, so scaling from it would make Shield Block's Block chance drift every
    /// time an attack was blocked and would make Last Stand a bonus derived from a number it
    /// had just increased. What a player means by "my health pool" for the purposes of gear
    /// is this one.
    #[must_use]
    pub fn scaling_health(&self) -> f32 {
        health_from_stamina(self.character_stats().get(Stat::Stamina))
    }

    /// The player's current health.
    #[must_use]
    pub fn player_health(&self) -> f32 {
        self.world
            .health(self.player.entity)
            .map_or(0.0, crate::combat::Health::current)
    }

    /// **Pushes the Warrior's live multipliers into the world's maximum health.**
    ///
    /// Called whenever Deflection moves, Last Stand starts or ends, or a tick passes. The
    /// current health is *kept* rather than scaled — see
    /// [`crate::combat::Health::set_max_keeping_current`] — because a resource that moves by
    /// two tenths every time an attack is blocked would otherwise be a stream of free
    /// healing.
    pub(super) fn refresh_warrior_health(&mut self) {
        let stats = self.character_stats();
        let base = health_from_stamina(stats.get(Stat::Stamina));
        let max = base * self.warrior_health_multiplier();
        self.world
            .set_max_health_keeping_current(self.player.entity, max);
    }

    /// The maximum health multiplier the current hero has. One for everybody but the Warrior.
    pub(super) fn warrior_health_multiplier(&self) -> f32 {
        if self.hero == Hero::Warrior {
            self.warrior.max_health_multiplier()
        } else {
            1.0
        }
    }

    /// **Spends this ability's Deflection, if there is any to spend.**
    ///
    /// Returns whether the ability is empowered. An ability whose row asks for twenty and
    /// finds nineteen is cast in its normal form: it is not refused, and nothing is
    /// part-paid. The passive maximum health and armour bonuses are recomputed *before* the
    /// event is emitted, so "spending gives the defence straight back" is true of the very
    /// same call rather than of the next tick.
    fn spend_deflection(&mut self, ability: FireAbility) -> bool {
        let cost = ability.definition().warrior.deflection_cost;
        self.spend_deflection_amount(ability, cost)
    }

    /// The same, for a spend whose size is a rule rather than a column — Recuperate's.
    fn spend_deflection_amount(&mut self, ability: FireAbility, cost: f32) -> bool {
        if self.hero != Hero::Warrior || cost <= 0.0 {
            return false;
        }
        if !self.warrior.deflection.spend(cost) {
            return false;
        }
        self.refresh_warrior_health();
        let tick = self.tick;
        let value = self.warrior.deflection.value();
        self.events.push(CombatEvent::DeflectionSpent {
            entity: self.player.entity,
            amount: cost,
            value,
            ability: AbilityId::from_fire(ability),
            tick,
        });
        true
    }

    /// **Applies a Warrior ability whose targets have already been collected.**
    ///
    /// Reached from [`super::Simulation::land`], which has already spent the charge, faced
    /// the cast and emitted `AbilityUsed`. Everything class-specific starts here.
    pub(super) fn land_warrior(&mut self, ability: FireAbility, hits: usize) -> u32 {
        match ability {
            FireAbility::Recuperate => self.cast_recuperate(),
            FireAbility::WarStomp => self.cast_war_stomp(),
            FireAbility::ColossusStrike => self.cast_colossus_strike(),
            FireAbility::ShieldCharge => self.cast_shield_charge(),
            FireAbility::ShieldBash => self.cast_shield_bash(),
            FireAbility::ShieldBlock => self.cast_shield_block(),
            FireAbility::PunctureBlood => self.cast_puncture_blood(),
            FireAbility::Adrenaline => self.cast_adrenaline(),
            FireAbility::BattleTrance => self.cast_battle_trance(),
            FireAbility::SpellReflect => self.cast_spell_reflect(),
            FireAbility::LastStand => self.cast_last_stand(),
            _ => hits as u32,
        }
    }

    /// Recuperate: swing, and stack the heal.
    ///
    /// The one ability whose Deflection interaction is a *rule* rather than a cost: at a
    /// full hundred it spends twenty and applies two stacks, which is the outlet for a
    /// player who has nothing else to spend on. Below a hundred it is unchanged, so normal
    /// Recuperate gameplay never has to think about the resource.
    fn cast_recuperate(&mut self) -> u32 {
        let ability = FireAbility::Recuperate;
        let definition = *ability.definition();
        let stats = self.mage.stats();
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            self.strike(target, definition.base_damage(&stats), ability, true, 0.0);
            affected += 1;
        }
        if affected == 0 {
            return 0;
        }

        let capped = self.hero == Hero::Warrior && self.warrior.deflection.is_full();
        let stacks = if capped && self.spend_deflection_amount(ability, DEFLECTION_SPEND) {
            2
        } else {
            1
        };
        let source = self.player.entity;
        let status = Status::new(
            StatusKind::Recuperating,
            source,
            AbilityId::from_fire(ability),
            definition.duration,
            0.0,
        )
        .every(definition.tick_interval)
        .scaling_with_source_health(RECUPERATE_HEAL_PER_STACK)
        .with_stacks(stacks);
        self.apply_status(source, status, definition.duration);
        affected
    }

    /// War Stomp: a hundred and twenty degrees of it, and the armour that comes with a full
    /// stack.
    fn cast_war_stomp(&mut self) -> u32 {
        let ability = FireAbility::WarStomp;
        let definition = *ability.definition();
        let empowered = self.spend_deflection(ability);
        let stacks = if empowered { 2 } else { 1 };
        let stats = self.mage.stats();
        let source = self.player.entity;
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            self.strike(target, definition.base_damage(&stats), ability, true, 0.0);
            if !self.world.is_alive(target) {
                continue;
            }
            let status = Status::new(
                StatusKind::Sundered,
                source,
                AbilityId::from_fire(ability),
                definition.duration,
                0.0,
            )
            .with_stacks(stacks);
            self.apply_status(target, status, definition.duration);
            affected += 1;
        }
        // The debuff is a number to the enemy's mind, pushed in immediately rather than on
        // the next tick, so a stomp slows the swing that was about to happen.
        self.sync_war_stomp();
        affected
    }

    /// Colossus Strike: an area hit that reads maximum health, and a bleed that keeps
    /// reading it.
    fn cast_colossus_strike(&mut self) -> u32 {
        let ability = FireAbility::ColossusStrike;
        let definition = *ability.definition();
        let empowered = self.spend_deflection(ability);
        let bleed_scale = if empowered {
            1.0 + COLOSSUS_BLEED_EMPOWERED
        } else {
            1.0
        };
        let stats = self.mage.stats();
        let max_health = self.player_max_health();
        let impact = definition.base_damage_with_health(&stats, max_health);
        // The bleed's *coefficients*, not its damage: the flat half is decided now because
        // it is a share of attack power, and the health half is deliberately left as a
        // fraction so that every tick reads the Warrior's maximum health at the moment it
        // happens. See `Status::source_health_per_tick`.
        let tick_flat = definition.base_tick_damage(&stats) * bleed_scale;
        let tick_health = definition.warrior.tick_health_coefficient * bleed_scale;
        let source = self.player.entity;
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            self.strike(target, impact, ability, true, 0.0);
            if !self.world.is_alive(target) {
                continue;
            }
            let status = Status::new(
                StatusKind::Bleeding,
                source,
                AbilityId::from_fire(ability),
                definition.duration,
                tick_flat,
            )
            .every(definition.tick_interval)
            .scaling_with_source_health(tick_health);
            self.apply_status(target, status, definition.duration);
            affected += 1;
        }
        affected
    }

    /// Shield Charge: **starts running.**
    ///
    /// Nothing lands here. The Warrior is given a destination and the movement stage walks
    /// them to it at [`SHIELD_CHARGE_SPEED`] metres a second, through the same collision
    /// world every other step goes through — which is the difference between a charge and a
    /// Blink, and the reason the two are no longer the same `dash`.
    fn cast_shield_charge(&mut self) -> u32 {
        let ability = FireAbility::ShieldCharge;
        let Some(target) = self.player.target.filter(|id| self.world.is_alive(*id)) else {
            return 0;
        };
        let Some(destination) = self.world.position(target) else {
            return 0;
        };
        let empowered = self.spend_deflection(ability);
        self.warrior.charge = Some(crate::warrior::ChargeMotion {
            target,
            destination,
            elapsed: 0.0,
            empowered,
        });
        0
    }

    /// Shield Bash: the interrupt, and the silence twenty Deflection buys.
    fn cast_shield_bash(&mut self) -> u32 {
        let ability = FireAbility::ShieldBash;
        let definition = *ability.definition();
        let empowered = self.spend_deflection(ability);
        let stats = self.mage.stats();
        let Some(target) = self.hits.first().copied() else {
            return 0;
        };
        self.strike(target, definition.base_damage(&stats), ability, true, 0.0);
        // The interrupt is the existing one, with the existing rules about what may be
        // interrupted. A second implementation would be a second answer to "can this cast be
        // stopped".
        self.interrupt(target);
        if empowered && self.is_enemy(target) {
            // Enemies only, and never the Warrior: a silence is applied to the thing that
            // was bashed, and the only thing that can be bashed is something hostile.
            if let Some(Some(runtime)) = self.enemies.get_mut(target.index()) {
                runtime.brain.silence(SHIELD_BASH_SILENCE);
            }
            let tick = self.tick;
            self.events.push(CombatEvent::ControlApplied {
                source: self.player.entity,
                target,
                control: ControlKind::Silence,
                seconds: SHIELD_BASH_SILENCE,
                ability: AbilityId::from_fire(ability),
                tick,
            });
        }
        1
    }

    /// Shield Block: six seconds of Block, and no critical strikes taken.
    fn cast_shield_block(&mut self) -> u32 {
        let ability = FireAbility::ShieldBlock;
        let definition = *ability.definition();
        let empowered = self.spend_deflection(ability);
        self.warrior.shield_block_empowered = empowered;
        self.warrior.shield_block = crate::firemage::Timer::idle();
        self.warrior.shield_block.start(definition.duration);
        0
    }

    /// Puncture Blood: current health for an absorb built out of maximum health.
    ///
    /// The two halves read **different numbers on purpose**: the cost is a share of what the
    /// Warrior has left, and the shield is a share of what they could have. A Warrior at
    /// eight thousand of ten thousand pays 2,400 and gets 3,000, which is what makes it an
    /// emergency button rather than a rotational one.
    fn cast_puncture_blood(&mut self) -> u32 {
        let ability = FireAbility::PunctureBlood;
        let definition = *ability.definition();
        let empowered = self.spend_deflection(ability);
        let entity = self.player.entity;
        let current = self.player_health();
        let max = self.player_max_health();
        let cost = current * PUNCTURE_HEALTH_COST;
        let scale = if empowered {
            PUNCTURE_EMPOWERED_SCALE
        } else {
            1.0
        };
        let shield = max * PUNCTURE_SHIELD * scale;

        // Applied straight to the health rather than through `deal_damage`: this is the
        // Warrior cutting themselves, not an attack. Running it through the incoming
        // pipeline would let their own armour reduce it, their own shield absorb it and
        // their own Deflection be generated by it — three ways for the cost to be avoided.
        let position = self.world.position(entity).unwrap_or(Vec3::ZERO);
        let result = self.world.damage(entity, cost);
        let tick = self.tick;
        if result.applied > 0.0 {
            self.events
                .push(CombatEvent::Damage(crate::event::DamageEvent {
                    source: entity,
                    target: entity,
                    amount: result.applied,
                    overkill: result.overkill,
                    ability: AbilityId::from_fire(ability),
                    damage_type: DamageType::Physical,
                    killed: result.killed,
                    critical: false,
                    position,
                    tick,
                }));
        }

        self.warrior.absorb.grant(shield, definition.duration);
        self.events.push(CombatEvent::AbsorbApplied {
            target: entity,
            amount: shield,
            duration: definition.duration,
            ability: AbilityId::from_fire(ability),
            tick,
        });
        1
    }

    /// Adrenaline: start banking a share of the damage taken.
    fn cast_adrenaline(&mut self) -> u32 {
        let ability = FireAbility::Adrenaline;
        let definition = *ability.definition();
        let empowered = self.spend_deflection(ability);
        self.warrior.adrenaline_share = if empowered {
            ADRENALINE_EMPOWERED_SHARE
        } else {
            ADRENALINE_SHARE
        };
        // A second Adrenaline is a fresh window, not a continuation: whatever the last one
        // banked has already been paid out.
        self.warrior.recovery_pool = 0.0;
        self.warrior.adrenaline = crate::firemage::Timer::idle();
        self.warrior.adrenaline.start(definition.duration);
        0
    }

    /// Battle Trance: ten seconds where defending heals and pays double.
    fn cast_battle_trance(&mut self) -> u32 {
        let definition = *FireAbility::BattleTrance.definition();
        self.warrior.battle_trance = crate::firemage::Timer::idle();
        self.warrior.start_battle_trance(definition.duration);
        0
    }

    /// Spell Reflect: six seconds of sending spells home.
    fn cast_spell_reflect(&mut self) -> u32 {
        let ability = FireAbility::SpellReflect;
        let definition = *ability.definition();
        let empowered = self.spend_deflection(ability);
        self.warrior.spell_reflect_empowered = empowered;
        self.warrior.spell_reflect = crate::firemage::Timer::idle();
        self.warrior.spell_reflect.start(definition.duration);
        0
    }

    /// Last Stand: more maximum health, a heal for the same share, and less damage taken.
    ///
    /// **The heal is computed from the maximum health before the buff**, once, and the buff's
    /// own bonus is held on the Warrior rather than recomputed. Both are the same guard
    /// against the same circularity: a bonus derived from a maximum health it has already
    /// increased would grow every time anything asked for it.
    fn cast_last_stand(&mut self) -> u32 {
        let ability = FireAbility::LastStand;
        let definition = *ability.definition();
        let empowered = self.spend_deflection(ability);
        let before = self.player_max_health();
        let bonus = health_scaled(
            LAST_STAND_BASE,
            LAST_STAND_HEALTH_SCALE,
            self.scaling_health(),
        );

        self.warrior.last_stand_bonus = bonus;
        self.warrior.last_stand_reduction = if empowered {
            LAST_STAND_EMPOWERED_REDUCTION
        } else {
            LAST_STAND_REDUCTION
        };
        self.warrior.last_stand = crate::firemage::Timer::idle();
        self.warrior.last_stand.start(definition.duration);
        self.refresh_warrior_health();

        // The pre-effect maximum, deliberately: the heal is "the same share again", not "the
        // same share of the bigger number".
        self.heal_self(bonus * before, ability)
    }

    /// Applies a status and announces it.
    pub(super) fn apply_status(&mut self, target: EntityId, status: Status, duration: f32) {
        if !self.world.is_alive(target) {
            return;
        }
        let source = status.source;
        let kind = status.kind;
        if let Some(statuses) = self.world.statuses_mut(target) {
            statuses.apply(status);
        }
        let tick = self.tick;
        self.events.push(CombatEvent::StatusApplied {
            source,
            target,
            status: kind,
            duration,
            tick,
        });
    }

    /// Whether an entity is an enemy with a mind.
    #[must_use]
    pub fn is_enemy(&self, entity: EntityId) -> bool {
        self.enemies
            .get(entity.index())
            .is_some_and(Option::is_some)
    }

    // ------------------------------------------------------------------ the incoming side

    /// **What an attack on the player actually does**, or `None` if nothing of it lands.
    ///
    /// The one place an incoming attack is answered. Everything that pays out on a defensive
    /// event reads the single [`DefenseOutcome`] this produces, which is what makes
    /// "duplicate combat events cannot generate duplicate Deflection" structural rather than
    /// a rule somebody has to remember.
    pub(super) fn defend_player(
        &mut self,
        attacker: EntityId,
        amount: f32,
        ability: AbilityId,
        damage_type: DamageType,
    ) -> Option<f32> {
        let defender = self.player.entity;
        let profile = self.defense_profile();

        if self.hero == Hero::Warrior
            && self.warrior.spell_reflect.is_active()
            && self.is_reflectable(ability)
        {
            self.reflect_spell(attacker, amount, ability, damage_type);
            return None;
        }

        let outcome = profile.roll(&mut self.mage.rng, damage_type);
        self.last_defense = Some(outcome);
        if outcome.is_defensive() {
            let position = self.world.position(defender).unwrap_or(Vec3::ZERO);
            let tick = self.tick;
            self.events.push(CombatEvent::AttackDefended {
                attacker,
                defender,
                outcome,
                ability,
                position,
                tick,
            });
            self.on_defensive_event(outcome);
        }

        let landing = profile.mitigate(amount, outcome, damage_type);
        if landing <= 0.0 {
            return None;
        }

        let (absorbed, remaining) = self.warrior.absorb.absorb(landing);
        if absorbed > 0.0 {
            let tick = self.tick;
            let left = self.warrior.absorb.remaining();
            self.events.push(CombatEvent::Absorbed {
                target: defender,
                amount: absorbed,
                remaining: left,
                tick,
            });
        }
        if remaining <= 0.0 {
            return None;
        }

        // Banked from what actually landed, so a hit that was blocked, mitigated and partly
        // absorbed banks what it really cost rather than what it threatened.
        self.warrior.bank_recovery(remaining);
        Some(remaining)
    }

    /// Everything a successful Block, Parry or Dodge pays out. Called once per attack.
    fn on_defensive_event(&mut self, _outcome: DefenseOutcome) {
        if self.hero != Hero::Warrior {
            return;
        }
        let entity = self.player.entity;

        let gain = self.warrior.deflection_per_event();
        let gained = self.warrior.deflection.gain(gain);
        if gained > 0.0 {
            self.refresh_warrior_health();
            let tick = self.tick;
            let value = self.warrior.deflection.value();
            self.events.push(CombatEvent::DeflectionGained {
                entity,
                amount: gained,
                value,
                tick,
            });
        }

        // Battle Trance: one per cent of maximum health, three times a second, with no
        // ceiling on the total.
        if self.warrior.take_trance_activation() {
            let heal = self.player_max_health() * BATTLE_TRANCE_HEAL;
            self.heal_self(heal, FireAbility::BattleTrance);
        }

        // Spell Reflect: a tenth of a second off, and nothing at all if it is ready — the
        // reduction is not banked against the next cast.
        if !self.mage.abilities.is_ready(FireAbility::SpellReflect) {
            self.reduce_cooldown(
                FireAbility::SpellReflect,
                SPELL_REFLECT_COOLDOWN_PER_EVENT,
                f32::MAX,
            );
        }
    }

    /// Whether an incoming ability is a spell that may be sent back.
    ///
    /// Uses the classification the enemy table already has rather than a list of its own: a
    /// spell is an enemy ability **with a cast bar** whose damage is **not physical** and
    /// whose effect is plain damage. A swing is not a spell, burning ground is not a spell
    /// that can be handed back to anybody, and a heal is not something to reflect. Nothing a
    /// friendly source produces reaches here at all — the caller has already established
    /// that the attacker is hostile.
    #[must_use]
    pub fn is_reflectable(&self, ability: AbilityId) -> bool {
        let Some(enemy) = ability.enemy() else {
            return false;
        };
        let definition = enemy.definition();
        definition.is_cast()
            && !matches!(definition.damage_type, DamageType::Physical)
            && matches!(definition.effect, EnemyEffect::Damage)
    }

    /// Sends a spell back to whoever cast it.
    fn reflect_spell(
        &mut self,
        attacker: EntityId,
        amount: f32,
        ability: AbilityId,
        damage_type: DamageType,
    ) {
        let empowered = self.warrior.spell_reflect_empowered;
        // Consumed by the reflection it paid for, not by the buff running out: "the next
        // successfully reflected spell" is exactly one spell.
        self.warrior.spell_reflect_empowered = false;
        let scale = if empowered {
            1.0 + SPELL_REFLECT_EMPOWERED_DAMAGE
        } else {
            1.0
        };
        let reflected = amount * scale;
        let tick = self.tick;
        self.events.push(CombatEvent::SpellReflected {
            source: self.player.entity,
            target: attacker,
            ability,
            amount: reflected,
            empowered,
            tick,
        });
        // Credited to Spell Reflect rather than to the enemy's own row, so the damage meter
        // shows the ability the player pressed.
        self.deal_damage(
            self.player.entity,
            attacker,
            reflected,
            AbilityId::from_fire(FireAbility::SpellReflect),
            damage_type,
            false,
        );
    }

    // ------------------------------------------------------------------ the tick

    /// Everything the Warrior does on a tick that nothing else asked for.
    pub(super) fn tick_warrior(&mut self, step: f32) {
        if self.hero != Hero::Warrior {
            return;
        }
        self.tick_shield_charge(step);
        let adrenaline_ended = self.warrior.tick(step);
        if adrenaline_ended {
            self.start_recovery();
        }
        self.sync_war_stomp();
        // Last, because everything above can have changed a multiplier.
        self.refresh_warrior_health();
    }

    /// Turns the banked Recovery Pool into healing over the following six seconds.
    fn start_recovery(&mut self) {
        let pool = self.warrior.take_recovery_pool();
        if pool <= 0.0 {
            return;
        }
        let entity = self.player.entity;
        let status = Status::new(
            StatusKind::Recovering,
            entity,
            AbilityId::from_fire(FireAbility::Adrenaline),
            ADRENALINE_RECOVERY_SECONDS,
            pool / ADRENALINE_RECOVERY_SECONDS,
        );
        self.apply_status(entity, status, ADRENALINE_RECOVERY_SECONDS);
    }

    /// Walks a Shield Charge one step, and resolves it if it has arrived.
    fn tick_shield_charge(&mut self, step: f32) {
        let Some(mut charge) = self.warrior.charge else {
            return;
        };
        let entity = self.player.entity;
        if !self.world.is_alive(entity) {
            self.warrior.charge = None;
            return;
        }
        charge.elapsed += step;
        // A living target that has moved is followed; a dead one leaves the destination it
        // was standing on, so a charge into a pack that lost its primary still lands.
        if let Some(position) = self
            .world
            .is_alive(charge.target)
            .then(|| self.world.position(charge.target))
            .flatten()
        {
            charge.destination = position;
        }

        let origin = self.world.position(entity).unwrap_or(Vec3::ZERO);
        let offset = Vec3::new(
            charge.destination.x - origin.x,
            0.0,
            charge.destination.z - origin.z,
        );
        let distance = (offset.x * offset.x + offset.z * offset.z).sqrt();

        if distance <= SHIELD_CHARGE_CONTACT || charge.elapsed >= SHIELD_CHARGE_TIMEOUT {
            self.warrior.charge = None;
            self.resolve_shield_charge(charge);
            return;
        }

        let travel = (SHIELD_CHARGE_SPEED * step).min(distance);
        let direction = Vec3::new(offset.x / distance, 0.0, offset.z / distance);
        // The same collided step every other movement takes: a charge into a wall stops at
        // the wall, and the timeout ends it. This is what makes it a run rather than a Blink.
        self.slide_player(direction * travel);
        self.face_towards(charge.destination);
        self.warrior.charge = Some(charge);
    }

    /// The end of a Shield Charge: the area hit, the stun, and the threat.
    fn resolve_shield_charge(&mut self, charge: crate::warrior::ChargeMotion) {
        let ability = FireAbility::ShieldCharge;
        let definition = *ability.definition();
        let stats = self.mage.stats();
        let scale = if charge.empowered {
            1.0 + SHIELD_CHARGE_EMPOWERED_DAMAGE
        } else {
            1.0
        };
        let base = definition.base_damage(&stats) * scale;

        self.hits.clear();
        self.collect_in_shape(
            charge.destination,
            ZoneShape::Circle {
                radius: definition.radius,
            },
        );
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            self.strike(target, base, ability, true, 0.0);
            affected += 1;
        }

        // The stun is the primary target's alone, and its duration is never changed by
        // Deflection: what twenty buys is damage.
        if self.world.is_alive(charge.target)
            && let Some(Some(runtime)) = self.enemies.get_mut(charge.target.index())
        {
            runtime.brain.stun(SHIELD_CHARGE_STUN);
            let tick = self.tick;
            self.events.push(CombatEvent::ControlApplied {
                source: self.player.entity,
                target: charge.target,
                control: ControlKind::Stun,
                seconds: SHIELD_CHARGE_STUN,
                ability: AbilityId::from_fire(ability),
                tick,
            });
        }

        let tick = self.tick;
        self.events.push(CombatEvent::ImpactLanded {
            source: self.player.entity,
            ability: AbilityId::from_fire(ability),
            position: charge.destination,
            radius: definition.radius,
            targets: affected,
            tick,
        });
    }

    /// Pushes War Stomp's stacks into the enemies' minds, and reads the armour condition
    /// back out.
    ///
    /// Recomputed from the statuses rather than counted as they are applied: a stack that
    /// expired, an enemy that died and an enemy that was never stomped all answer the same
    /// way, on the tick it becomes true.
    pub(super) fn sync_war_stomp(&mut self) {
        let mut full_stack = false;
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            let alive = self.world.is_alive(id);
            let stacks = if alive {
                self.world
                    .statuses(id)
                    .map_or(0, |statuses| statuses.stacks(StatusKind::Sundered))
            } else {
                0
            };
            if stacks >= StatusKind::Sundered.max_stacks() {
                full_stack = true;
            }
            if let Some(Some(runtime)) = self.enemies.get_mut(slot) {
                let scale = 1.0 - WAR_STOMP_ATTACK_SPEED_PER_STACK * f32::from(stacks);
                runtime.brain.set_attack_speed_scale(scale);
            }
        }
        self.warrior.war_stomp_armor = full_stack;
    }
}
