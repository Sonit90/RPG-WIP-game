//! Casting the Warrior: Valor in, Battle Fervor out, and the eleven abilities between.
//!
//! ```text
//! an attack arrives
//!      │
//!      ├─ Spell Reflect?  ──► back to the caster, and nothing lands
//!      │
//!      ├─ DefenseProfile::roll  (one roll, one outcome)
//!      │        └─ defensive? ──┬─ Battle Trance: 1 % max HP, three times a second
//!      │                        └─ Spell Reflect: −0.1 s off its cooldown
//!      ├─ mitigate: block share, armour, flat reduction
//!      ├─ absorb: Puncture Blood's shield stands in front of health
//!      └─ Adrenaline banks its share of what actually landed
//! ```
//!
//! And on the outgoing side, a War Stomp, a Colossus Strike or a Shield Charge that lands pays
//! one Valor; the fifth empties the bar and applies a stack of Battle Fervor.
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
use crate::talent::warrior::WarriorTalentEffects;
use crate::warrior::health_scaled;
use crate::warrior::tuning::*;

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

    /// What the active Warrior's talents mean, derived from the grid on every purchase.
    #[must_use]
    pub fn warrior_effects(&self) -> &WarriorTalentEffects {
        &self.warrior_effects
    }

    /// Pushes the derived talent effects into the state they live in: the Valor bar's
    /// length and the ability book's extra charges and cooldowns.
    pub(super) fn refresh_warrior_talents(&mut self) {
        let fx = self.warrior_effects;
        let effects = *self.mage.effects();
        self.warrior.valor.set_capacity(fx.valor_capacity);
        let book = &mut self.mage.abilities;
        book.adjust(
            FireAbility::ShieldCharge,
            fx.shield_charge_extra_charges,
            0.0,
            &effects,
        );
        book.adjust(
            FireAbility::WarStomp,
            0,
            fx.war_stomp_cooldown_extra,
            &effects,
        );
        book.adjust(
            FireAbility::ColossusStrike,
            0,
            fx.colossus_cooldown_extra,
            &effects,
        );
        book.adjust(
            FireAbility::LastStand,
            0,
            fx.last_stand_cooldown_extra,
            &effects,
        );
    }

    /// How far `ability` reaches for the active hero: its row, and Shockwave's wider stomp.
    #[must_use]
    pub fn ability_range(&self, ability: FireAbility) -> f32 {
        let range = ability.definition().range;
        if ability == FireAbility::WarStomp && self.hero == Hero::Warrior {
            range * self.warrior_effects.war_stomp_range_scale
        } else {
            range
        }
    }

    /// The factors Fervent Guard puts on Battle Fervor's damage dealt and damage taken for
    /// `entity`: `(1.0, 1.0)` for anybody but a Warrior, or without the talent.
    pub(super) fn fervor_corrections(&self, entity: EntityId) -> (f32, f32) {
        if self.hero != Hero::Warrior || entity != self.player.entity {
            return (1.0, 1.0);
        }
        let stacks = self
            .world
            .statuses(entity)
            .map_or(0, |statuses| statuses.stacks(StatusKind::BattleFervor));
        if stacks == 0 {
            return (1.0, 1.0);
        }
        (
            self.warrior_effects.fervor_dealt_correction(stacks),
            self.warrior_effects.fervor_taken_correction(stacks),
        )
    }

    /// Executioner: what one Colossus Strike hit on `target` is multiplied by, by how hurt
    /// the target is right now.
    pub(super) fn executioner_multiplier(&self, target: EntityId) -> f32 {
        let fx = &self.warrior_effects;
        if fx.executioner_bonus <= 0.0 {
            return 1.0;
        }
        let below = self.world.health(target).is_some_and(|health| {
            health.max() > 0.0 && health.current() / health.max() < fx.executioner_threshold
        });
        if below {
            1.0 + fx.executioner_bonus
        } else {
            1.0
        }
    }

    /// **What an attack on the player meets right now.**
    ///
    /// Derived on every call and never stored: the Warrior's armour changes on the tick War
    /// Stomp reaches a full stack, and their Block changes on the tick Shield Block starts. A stored
    /// profile would be a snapshot of the one thing in the class that must not be one.
    #[must_use]
    pub fn defense_profile(&self) -> DefenseProfile {
        let stats = self.character_stats();
        let mut profile = DefenseProfile::from_stats(&stats);
        if self.hero != Hero::Warrior {
            // Every other hero's own reduction, through the same field the Warrior's Last
            // Stand uses: one number, applied once, where damage is mitigated.
            profile.damage_reduction = self.status_damage_reduction(self.player.entity);
            return profile;
        }
        profile.armor *= self.warrior.armor_multiplier();
        profile.block += self.warrior.shield_block_bonus(self.scaling_health());
        profile.immune_to_critical = self.warrior.immune_to_critical();
        // The class's own reduction and whatever the player's effects give, compounded: two
        // reductions of thirty per cent leave forty-nine, not forty.
        let effects = self.status_damage_reduction(self.player.entity);
        profile.damage_reduction = 1.0 - (1.0 - self.warrior.damage_reduction()) * (1.0 - effects);
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
    /// the same reason: the live maximum already contains Last Stand's and Battle
    /// Fervor's own, so scaling from it would make Shield Block's Block chance drift every
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
    /// Called whenever Last Stand or Battle Fervor starts or ends, or a tick passes. The
    /// current health is *kept* rather than scaled — see
    /// [`crate::combat::Health::set_max_keeping_current`] — because a buff that comes and goes
    /// mid-fight would otherwise be a stream of free healing.
    pub(super) fn refresh_warrior_health(&mut self) {
        let stats = self.character_stats();
        let base = health_from_stamina(stats.get(Stat::Stamina));
        let max = self
            .health_override
            .unwrap_or(base * self.warrior_health_multiplier());
        self.world
            .set_max_health_keeping_current(self.player.entity, max);
    }

    /// The maximum health multiplier the current hero has: the Warrior's own (Last Stand), and
    /// whatever the effects on the player multiply it by — Battle Fervor's per-stack share.
    pub(super) fn warrior_health_multiplier(&self) -> f32 {
        let class = if self.hero == Hero::Warrior {
            self.warrior.max_health_multiplier()
        } else {
            1.0
        };
        let effects = self
            .world
            .statuses(self.player.entity)
            .map_or(1.0, crate::status::StatusSet::max_health_multiplier);
        class * effects
    }

    /// **Pays this ability's Valor, if its row pays any.**
    ///
    /// One call per successful cast, whatever the number of targets, so a stomp that caught
    /// five enemies pays what a stomp that caught one does. When the stack is the fifth the
    /// bar is already empty on return and one stack of Battle Fervor has been applied:
    /// filling the bar and being rewarded for it are one operation.
    fn gain_valor(&mut self, ability: FireAbility) {
        let row = ability.definition().warrior();
        if self.hero != Hero::Warrior || row.valor <= 0.0 {
            return;
        }
        self.add_valor(ability, 1);
    }

    /// Adds `amount` Valor one stack at a time, so each stack is announced and the one that
    /// fills the bar pays its Battle Fervor on the call that gains it.
    fn add_valor(&mut self, ability: FireAbility, amount: u8) {
        for _ in 0..amount {
            let filled = self.warrior.valor.gain();
            let tick = self.tick;
            self.events.push(CombatEvent::ValorGained {
                entity: self.player.entity,
                value: self.warrior.valor.stacks(),
                filled,
                ability: AbilityId::from_fire(ability),
                tick,
            });
            if filled {
                self.apply_battle_fervor(ability, 1);
            }
        }
    }

    /// Bloodied Valor: the final tick of a Colossus Strike bleed pays Valor.
    pub(super) fn grant_bloodied_valor(&mut self) {
        let amount = self.warrior_effects.bloodied_valor;
        if self.hero == Hero::Warrior && amount > 0 {
            self.add_valor(FireAbility::ColossusStrike, amount);
        }
    }

    /// Adds `stacks` of Battle Fervor to the Warrior and refreshes the whole status.
    ///
    /// The stack count is capped by the status's own ceiling ([`FERVOR_MAX_STACKS`]), and the
    /// duration restarts at [`FERVOR_DURATION`] whether or not the stack count could rise: a
    /// full Fervor is still worth refreshing. Every stack granted also pays Bulwark Rhythm.
    fn apply_battle_fervor(&mut self, ability: FireAbility, stacks: u8) {
        let source = self.player.entity;
        let status = Status::new(
            StatusKind::BattleFervor,
            source,
            AbilityId::from_fire(ability),
            FERVOR_DURATION,
            0.0,
        )
        .with_stacks(stacks);
        self.apply_status(source, status, FERVOR_DURATION);
        // The maximum-health share, if it is tuned above zero, is there on this tick.
        self.refresh_warrior_health();
        let cut = self.warrior_effects.bulwark_cooldown_cut * f32::from(stacks);
        self.reduce_cooldown(FireAbility::ShieldBlock, cut, f32::MAX);
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
            FireAbility::Taunt => self.cast_taunt(),
            FireAbility::LastStand => self.cast_last_stand(),
            _ => hits as u32,
        }
    }

    /// Recuperate: swing, and stack the heal.
    ///
    /// Pays no Valor: it is the Warrior's basic strike, and the bar belongs to the three
    /// heavy abilities.
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
        .with_stacks(1);
        self.apply_status(source, status, definition.duration);
        affected
    }

    /// War Stomp: a hundred and twenty degrees of it, and the armour that comes with a full
    /// stack.
    fn cast_war_stomp(&mut self) -> u32 {
        let ability = FireAbility::WarStomp;
        let definition = *ability.definition();
        // Nothing caught, nothing paid: a stomp into empty air earns no Valor.
        if self.hits.is_empty() {
            return 0;
        }
        let stats = self.mage.stats();
        let source = self.player.entity;
        let damage = definition.base_damage(&stats) * self.warrior_effects.war_stomp_damage_scale;
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            self.strike(target, damage, ability, true, 0.0);
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
            .with_stacks(1 + self.warrior_effects.war_stomp_extra_stacks);
            self.apply_status(target, status, definition.duration);
            affected += 1;
        }
        self.gain_valor(ability);
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
        // A smash with nobody in reach earns no Valor.
        if self.hits.is_empty() {
            return 0;
        }
        let stats = self.mage.stats();
        let max_health = self.player_max_health();
        let impact = definition.base_damage_with_health(&stats, max_health);
        // The bleed's *coefficients*, not its damage: the flat half is decided now because
        // it is a share of attack power, and the health half is deliberately left as a
        // fraction so that every tick reads the Warrior's maximum health at the moment it
        // happens. See `Status::source_health_per_tick`.
        let tick_flat = definition.base_tick_damage(&stats);
        let tick_health = definition.warrior().tick_health_coefficient;
        let source = self.player.entity;
        let bleeding = self.bleed_targets();
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            // Executioner: a target already under the threshold takes the bonus on impact.
            let multiplier = self.executioner_multiplier(target);
            self.strike(target, impact * multiplier, ability, true, 0.0);
            if !self.world.is_alive(target) {
                continue;
            }
            // The smash cleaves everything; the bleed only takes hold on a few.
            if !bleeding.contains(&target) {
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
        self.gain_valor(ability);
        affected
    }

    /// **Who Colossus Strike's bleed lands on**: at most
    /// [`COLOSSUS_BLEED_MAX_TARGETS`] of the hit list, the selected target first and then the
    /// nearest to the Warrior.
    ///
    /// The direct hit still cleaves every enemy in the smash; capping only the damage over
    /// time keeps a pack from paying the bleed's whole value once per enemy.
    fn bleed_targets(&self) -> Vec<EntityId> {
        let origin = self
            .world
            .position(self.player.entity)
            .unwrap_or(Vec3::ZERO);
        let primary = self.player.target.filter(|id| self.hits.contains(id));
        let mut rest: Vec<(f32, usize, EntityId)> = self
            .hits
            .iter()
            .enumerate()
            .filter(|(_, id)| Some(**id) != primary)
            .map(|(order, id)| {
                let distance = self
                    .world
                    .position(*id)
                    .map_or(f32::MAX, |at| super::horizontal_distance(origin, at));
                (distance, order, *id)
            })
            .collect();
        // The hit list's order breaks a tie, so the choice never depends on the sort.
        rest.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        primary
            .into_iter()
            .chain(rest.into_iter().map(|(_, _, id)| id))
            .take(COLOSSUS_BLEED_MAX_TARGETS)
            .collect()
    }

    /// Shield Charge: **starts running.**
    ///
    /// Nothing lands here, and no Valor is paid: that happens on arrival. The Warrior is given a destination and the movement stage walks
    /// them to it at [`SHIELD_CHARGE_SPEED`] metres a second, through the same collision
    /// world every other step goes through — which is the difference between a charge and a
    /// Blink, and the reason the two are no longer the same `dash`.
    fn cast_shield_charge(&mut self) -> u32 {
        let Some(target) = self.player.target.filter(|id| self.world.is_alive(*id)) else {
            return 0;
        };
        let Some(destination) = self.world.position(target) else {
            return 0;
        };
        self.warrior.charge = Some(crate::warrior::ChargeMotion {
            target,
            destination,
            elapsed: 0.0,
        });
        0
    }

    /// Shield Bash: damage, and the interrupt.
    fn cast_shield_bash(&mut self) -> u32 {
        let ability = FireAbility::ShieldBash;
        let definition = *ability.definition();
        let Some(target) = self.hits.first().copied() else {
            return 0;
        };
        let stats = self.mage.stats();
        self.strike(target, definition.base_damage(&stats), ability, true, 0.0);
        // The interrupt is the existing one, with the existing rules about what may be
        // interrupted. A second implementation would be a second answer to "can this cast be
        // stopped".
        self.interrupt(target);
        1
    }

    /// Taunt: the target attacks the Warrior for the ability's duration.
    ///
    /// No damage and no Valor. The enemy's threat table does the rest: the taunt forces
    /// its target and lifts the Warrior to the top of its threat, and the switch itself is
    /// announced by the AI the next time the enemy thinks, like every other change of target.
    fn cast_taunt(&mut self) -> u32 {
        let ability = FireAbility::Taunt;
        let seconds = ability.definition().duration;
        let source = self.player.entity;
        let mut affected = 0;
        for index in 0..self.hits.len() {
            let target = self.hits[index];
            if !self.is_enemy(target) || !self.world.is_alive(target) {
                continue;
            }
            let Some(Some(runtime)) = self.enemies.get_mut(target.index()) else {
                continue;
            };
            runtime.brain.taunt(source, seconds);
            let tick = self.tick;
            self.events.push(CombatEvent::ControlApplied {
                source,
                target,
                control: ControlKind::Taunt,
                seconds,
                ability: AbilityId::from_fire(ability),
                tick,
            });
            affected += 1;
        }
        affected
    }

    /// Shield Block: six seconds of Block, and no critical strikes taken.
    fn cast_shield_block(&mut self) -> u32 {
        let ability = FireAbility::ShieldBlock;
        let definition = *ability.definition();
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
        let entity = self.player.entity;
        let current = self.player_health();
        let max = self.player_max_health();
        let cost = current * PUNCTURE_HEALTH_COST;
        let shield = max * PUNCTURE_SHIELD;

        // Applied straight to the health rather than through `deal_damage`: this is the
        // Warrior cutting themselves, not an attack. Running it through the incoming
        // pipeline would let their own armour reduce it, their own shield absorb it and
        // their own Block roll away from it — three ways for the cost to be avoided.
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
                    periodic: false,
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
        self.warrior.adrenaline_share = ADRENALINE_SHARE;
        // A second Adrenaline is a fresh window, not a continuation: whatever the last one
        // banked has already been paid out.
        self.warrior.recovery_pool = 0.0;
        self.warrior.adrenaline = crate::firemage::Timer::idle();
        self.warrior.adrenaline.start(definition.duration);
        0
    }

    /// Battle Trance: ten seconds where defending heals.
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
        let before = self.player_max_health();
        let bonus = health_scaled(
            LAST_STAND_BASE,
            LAST_STAND_HEALTH_SCALE,
            self.scaling_health(),
        );

        self.warrior.last_stand_bonus = bonus;
        self.warrior.last_stand = crate::firemage::Timer::idle();
        self.warrior.last_stand.start(definition.duration);
        self.refresh_warrior_health();

        // Unyielding: the stand is also a rallying cry.
        let stacks = self.warrior_effects.last_stand_fervor_stacks;
        if stacks > 0 {
            self.apply_battle_fervor(ability, stacks);
        }

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
    /// "duplicate combat events cannot pay a defensive event twice" structural rather than
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

        // The trinket's shield first, then the Warrior's own: one event, with what both
        // together have left.
        let (from_trinket, landing) = self.trinket.absorb.absorb(landing);
        if from_trinket > 0.0 && !self.trinket.absorb.is_active() {
            self.end_trinket_absorb_status(defender);
        }
        let (from_warrior, remaining) = self.warrior.absorb.absorb(landing);
        let absorbed = from_trinket + from_warrior;
        if absorbed > 0.0 {
            let tick = self.tick;
            let left = self.warrior.absorb.remaining() + self.trinket.absorb.remaining();
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
        let reflected = amount;
        let tick = self.tick;
        self.events.push(CombatEvent::SpellReflected {
            source: self.player.entity,
            target: attacker,
            ability,
            amount: reflected,
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
        let base = definition.base_damage(&stats) * self.warrior_effects.shield_charge_impact_scale;

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

        // The stun is the primary target's alone.
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

        // One Valor for the charge, paid on arrival and only if the impact hit something: a
        // charge that timed out against a wall earns nothing.
        if affected > 0 {
            self.gain_valor(ability);
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
                // Every attack-speed effect on it, War Stomp's included, from the registry.
                let scale = self
                    .world
                    .statuses(id)
                    .map_or(1.0, crate::status::StatusSet::attack_speed_scale);
                runtime.brain.set_attack_speed_scale(scale);
            }
        }
        self.warrior.war_stomp_armor = full_stack;
    }
}
