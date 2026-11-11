//! The Warrior: Valor, Battle Fervor, and the state the eleven abilities leave behind them.
//!
//! ```text
//!   War Stomp / Colossus Strike / Shield Charge          (one Valor per cast)
//!                       │
//!                       ▼
//!             Valor ○ ○ ○ ○ ○  ──── fifth pip ────► Valor 0, +1 Battle Fervor
//!                                                      (30 s, three stacks:
//!                                                       +10 % dealt, −3 % taken each)
//!
//!                 Block / Parry / Dodge          (character::defense, one roll per attack)
//!                          │
//!            ┌─────────────┴──────────────┐
//!            ▼                            ▼
//!      Battle Trance                 Spell Reflect
//!      1 % max HP (3 a second)       −0.1 s cooldown
//! ```
//!
//! # What is here and what is not
//!
//! This module is the Warrior's *state*: a resource, a handful of timers, an absorb, a
//! banked pool and a charge in flight. It is the mirror of [`crate::firemage::FireMage`],
//! and it is deliberately the same shape — the class that is not being played sits at rest
//! and costs a few floats.
//!
//! What is **not** here:
//!
//! * The abilities' numbers. They are this class's rows in [`abilities`], assembled into the
//!   one [`crate::ability::ABILITIES`] table, because there is one table and one executor.
//! * Dodge, Parry, Block and Armour. They are stats every character could have, and they
//!   live in [`crate::character::defense`] with the one roll that decides between them.
//! * Threat. Threat is damage, in one place, for everybody; the Warrior's rows carry a
//!   multiplier ([`crate::firemage::WarriorRow::threat_multiplier`]) and nothing else.
//! * Stun and silence. They are things done *to an enemy*, so they live on the enemy —
//!   see [`crate::enemy::brain::EnemyBrain`].
//!
//! # Tuning
//!
//! Every magnitude in the kit is a named constant in [`tuning`]. Nothing in the executor
//! writes a number down.

/// The Warrior's rows of the ability table.
pub mod abilities;
/// Valor: the resource that turns into Battle Fervor when it fills.
pub mod valor;

pub use valor::Valor;

use crate::entity::EntityId;
use crate::firemage::Timer;
use engine_core::math::Vec3;

/// Every magnitude the Warrior's kit is made of, in one place.
///
/// A designer changes the class here. Nothing in the executor, the interface or the tests
/// restates any of these numbers.
pub mod tuning {
    /// The maximum health a Warrior has with no gear at all.
    ///
    /// The reference the health-scaled abilities measure against, so "a Warrior with twice
    /// the health" is a number rather than an opinion. It is the same five hundred
    /// [`engine_core::tuning::player::MAX_HEALTH`] the enemy roster is tuned against, read
    /// from there rather than written down again.
    pub const BASE_MAX_HEALTH: f32 = engine_core::tuning::player::MAX_HEALTH;

    /// The health multiple the "fully geared" balance targets are stated at.
    ///
    /// One point six: a Warrior with sixty per cent more maximum health than the base. It is
    /// what turns Shield Block's `+6 %` into the design's `+24 %` and Last Stand's `+6 %`
    /// into its `+24 %`, through one formula each rather than two gear-specific numbers.
    /// Nothing reads it at runtime; the tests do, so that moving a scaling constant is
    /// visible as a change to the geared target rather than silent.
    pub const GEARED_HEALTH_MULTIPLE: f32 = 1.6;

    /// Valor: the most stacks the bar holds. The fifth is spent on the call that gains it.
    pub const VALOR_MAX: u8 = 5;

    /// Battle Fervor: seconds it lasts. Gaining a stack refreshes the whole status.
    pub const FERVOR_DURATION: f32 = 30.0;

    /// Battle Fervor: the most stacks it holds.
    pub const FERVOR_MAX_STACKS: u8 = 3;

    /// Battle Fervor: damage dealt gained per stack, as a fraction.
    pub const FERVOR_DAMAGE_PER_STACK: f32 = 0.10;

    /// Battle Fervor: damage taken lost per stack, as a fraction, of every damage type.
    pub const FERVOR_DAMAGE_TAKEN_PER_STACK: f32 = 0.03;

    /// Battle Fervor: maximum health gained per stack, as a fraction.
    ///
    /// Zero by default: the hook is there and tested, and a designer who wants it sets it to
    /// `0.01`.
    pub const FERVOR_MAX_HEALTH_PER_STACK: f32 = 0.0;

    /// Shield Charge: seconds the primary target is stunned.
    pub const SHIELD_CHARGE_STUN: f32 = 0.5;

    /// Shield Charge: metres a second the Warrior actually runs.
    ///
    /// A speed, because the charge is a movement rather than a teleport: the body travels
    /// through the collision world at this rate and arrives when it arrives.
    pub const SHIELD_CHARGE_SPEED: f32 = 22.0;

    /// Shield Charge: how close counts as having arrived, in metres.
    pub const SHIELD_CHARGE_CONTACT: f32 = 2.0;

    /// Shield Charge: the longest a charge may run before it gives up, in seconds.
    ///
    /// A charge whose target walked through a wall must end; without a ceiling the Warrior
    /// would be steered by a dead ability for the rest of the fight.
    pub const SHIELD_CHARGE_TIMEOUT: f32 = 3.0;

    /// Shield Block: the Block chance it grants before any health scaling.
    pub const SHIELD_BLOCK_BASE: f32 = 0.06;

    /// Shield Block: Block chance per whole multiple of base maximum health above the base.
    ///
    /// At [`GEARED_HEALTH_MULTIPLE`] this is `0.30 × 0.6 = +18 %`, which with
    /// [`SHIELD_BLOCK_BASE`] is the design's `+24 %`.
    pub const SHIELD_BLOCK_HEALTH_SCALE: f32 = 0.30;

    /// Battle Trance: the share of maximum health one defensive event restores.
    pub const BATTLE_TRANCE_HEAL: f32 = 0.01;

    /// Battle Trance: how many defensive events may heal in one second.
    ///
    /// Three. Not a cap on the total healing — there is none — but on the *rate*, so a pack
    /// of ten enemies swinging at once cannot pay out ten heals in a tick.
    pub const BATTLE_TRANCE_HEALS_PER_SECOND: u8 = 3;

    /// Last Stand: the maximum health it grants before any health scaling, as a fraction.
    pub const LAST_STAND_BASE: f32 = 0.06;

    /// Last Stand: extra maximum health per whole multiple of base maximum health above the
    /// base. At [`GEARED_HEALTH_MULTIPLE`] the two together are the design's `+24 %`.
    pub const LAST_STAND_HEALTH_SCALE: f32 = 0.30;

    /// Last Stand: damage taken reduction, as a fraction.
    pub const LAST_STAND_REDUCTION: f32 = 0.10;

    /// Spell Reflect: seconds off the remaining cooldown per successful defensive event.
    pub const SPELL_REFLECT_COOLDOWN_PER_EVENT: f32 = 0.1;

    /// Puncture Blood: the share of **current** health it costs.
    pub const PUNCTURE_HEALTH_COST: f32 = 0.30;

    /// Puncture Blood: the absorb it grants, as a share of **maximum** health.
    pub const PUNCTURE_SHIELD: f32 = 0.30;

    /// Adrenaline: the share of damage taken that is banked.
    pub const ADRENALINE_SHARE: f32 = 0.30;

    /// Adrenaline: seconds the banked pool is paid back over.
    pub const ADRENALINE_RECOVERY_SECONDS: f32 = 6.0;

    /// Recuperate: maximum health restored a second, per stack.
    ///
    /// Two tenths of a per cent; at the five-stack maximum, one per cent a second.
    pub const RECUPERATE_HEAL_PER_STACK: f32 = 0.002;

    /// War Stomp: attack speed lost per stack, as a fraction.
    pub const WAR_STOMP_ATTACK_SPEED_PER_STACK: f32 = 0.02;

    /// War Stomp: the armour the Warrior gains while something carries a full stack.
    pub const WAR_STOMP_ARMOR_BONUS: f32 = 0.10;

    /// Colossus Strike: the most enemies its bleed lands on (the selected target first, then
    /// the nearest). The direct hit still reaches everything in the smash.
    ///
    /// Without a cap the bleed, about three quarters of the ability's damage, is paid once
    /// per enemy in reach and the Warrior out-damages every other class against a pack.
    pub const COLOSSUS_BLEED_MAX_TARGETS: usize = 3;
}

use tuning::*;

/// A health bonus stated as "a base share, plus a share per multiple of base health".
///
/// **The one formula** behind Shield Block's Block chance and Last Stand's maximum health.
/// Both are specified as "a small number with no gear, a much larger one fully geared", and
/// both are written here as two tunable constants rather than as the two gear-specific
/// numbers they produce:
///
/// ```text
/// value = base + scale × (max_health / BASE_MAX_HEALTH − 1)
/// ```
///
/// At base gear the second term is zero and the answer is `base`. At
/// [`tuning::GEARED_HEALTH_MULTIPLE`] it is `base + 0.6 × scale`.
#[must_use]
pub fn health_scaled(base: f32, scale: f32, max_health: f32) -> f32 {
    let multiple = (max_health.max(0.0) / BASE_MAX_HEALTH).max(0.0);
    base + scale * (multiple - 1.0).max(0.0)
}

/// An absorb shield: a pool of damage that is taken instead of health, for a while.
///
/// Puncture Blood's, and the only one in the game. A pool with a clock rather than a status,
/// because what it does — stand in front of [`crate::combat::Health`] — is not something a
/// [`crate::status::StatusSet`] can express: a status ticks, and an absorb is consumed.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Absorb {
    amount: f32,
    timer: Timer,
}

impl Absorb {
    /// Nothing absorbed.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// How much is left to absorb.
    #[must_use]
    pub fn remaining(&self) -> f32 {
        if self.timer.is_active() {
            self.amount.max(0.0)
        } else {
            0.0
        }
    }

    /// Seconds before it lapses.
    #[must_use]
    pub fn seconds_left(&self) -> f32 {
        self.timer.remaining()
    }

    /// Whether anything is standing in front of the health.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.remaining() > 0.0
    }

    /// Replaces the shield with `amount` for `seconds`.
    ///
    /// Replaces rather than adds: two Puncture Bloods are one shield, and the second one is
    /// the one that counts. Adding them would make the ability worth pressing twice in a row
    /// on a full health bar, which is the opposite of what it is for.
    pub fn grant(&mut self, amount: f32, seconds: f32) {
        self.amount = amount.max(0.0);
        self.timer = Timer::idle();
        self.timer.start(seconds);
    }

    /// Takes what it can off `damage` and returns `(absorbed, remaining_damage)`.
    pub fn absorb(&mut self, damage: f32) -> (f32, f32) {
        let available = self.remaining();
        if available <= 0.0 || !damage.is_finite() || damage <= 0.0 {
            return (0.0, damage.max(0.0));
        }
        let taken = damage.min(available);
        self.amount = (self.amount - taken).max(0.0);
        if self.amount <= 0.0 {
            self.timer.clear();
        }
        (taken, damage - taken)
    }

    /// Advances the clock. Returns whether it lapsed on this call.
    pub fn tick(&mut self, seconds: f32) -> bool {
        let expired = self.timer.tick(seconds);
        if expired {
            self.amount = 0.0;
        }
        expired
    }

    /// Removes it.
    pub fn clear(&mut self) {
        self.amount = 0.0;
        self.timer.clear();
    }
}

/// A Shield Charge in flight.
///
/// The Warrior is *running*: this is a steering instruction the movement stage reads every
/// tick, not a destination it is placed at. See `Simulation::tick_shield_charge`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChargeMotion {
    /// Who is being charged.
    pub target: EntityId,
    /// Where they were when the charge started, so a target that dies mid-run still has a
    /// place to arrive at.
    pub destination: Vec3,
    /// Seconds the charge has been running, against [`tuning::SHIELD_CHARGE_TIMEOUT`].
    pub elapsed: f32,
}

/// Battle Trance's rate limit: three healing activations a second, deterministically.
///
/// Counted in whole seconds measured from the moment the buff started, rather than in a
/// sliding window: a window would make the answer depend on the order two attacks landed
/// within the same tick, and the limit has to be reproducible on a replay and on a server.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct TranceLimit {
    elapsed: f32,
    second: u32,
    used: u8,
}

impl TranceLimit {
    fn restart(&mut self) {
        *self = Self::default();
    }

    fn tick(&mut self, seconds: f32) {
        self.elapsed += seconds.max(0.0);
        // The epsilon is the project's usual guard against a fixed step of 1/60 leaving a
        // residue: without it the sixtieth tick reads 0.99999 and the second never turns
        // over. See `crate::TIME_EPSILON`.
        // `as` cannot truncate meaningfully: elapsed is a duration in seconds and the buff
        // is ten of them.
        let second = (self.elapsed + crate::TIME_EPSILON) as u32;
        if second != self.second {
            self.second = second;
            self.used = 0;
        }
    }

    fn take(&mut self) -> bool {
        if self.used >= BATTLE_TRANCE_HEALS_PER_SECOND {
            return false;
        }
        self.used += 1;
        true
    }
}

/// Everything the Warrior is, at rest or in a fight.
#[derive(Debug, Clone, PartialEq)]
pub struct Warrior {
    /// The resource.
    pub valor: Valor,
    /// Shield Block: seconds of extra Block and no critical strikes taken.
    pub shield_block: Timer,
    /// Battle Trance: seconds of healing defensive events.
    pub battle_trance: Timer,
    /// Last Stand: seconds of extra maximum health and reduced damage.
    pub last_stand: Timer,
    /// The maximum health Last Stand added, as a fraction, decided when it was cast.
    ///
    /// Held rather than recomputed, so the bonus does not chase its own tail: the value is
    /// derived from the maximum health *before* the buff, once.
    pub last_stand_bonus: f32,
    /// Spell Reflect: seconds enemy spells go back where they came from.
    pub spell_reflect: Timer,
    /// Adrenaline: seconds damage is being banked.
    pub adrenaline: Timer,
    /// The share of damage the running Adrenaline banks.
    pub adrenaline_share: f32,
    /// What Adrenaline has banked so far, waiting for the buff to end.
    pub recovery_pool: f32,
    /// Puncture Blood's shield.
    pub absorb: Absorb,
    /// A Shield Charge in flight, if there is one.
    pub charge: Option<ChargeMotion>,
    /// Whether something in the world currently carries a full stack of War Stomp.
    ///
    /// Recomputed every tick from the enemies' own statuses rather than counted as it is
    /// applied, so a stack that expired, or an enemy that died, takes the armour with it on
    /// the same tick.
    pub war_stomp_armor: bool,
    trance: TranceLimit,
}

impl Default for Warrior {
    fn default() -> Self {
        Self::new()
    }
}

impl Warrior {
    /// A Warrior at rest: no resource, no cooldowns running, nothing banked.
    #[must_use]
    pub fn new() -> Self {
        Self {
            valor: Valor::empty(),
            shield_block: Timer::idle(),
            battle_trance: Timer::idle(),
            last_stand: Timer::idle(),
            last_stand_bonus: 0.0,
            spell_reflect: Timer::idle(),
            adrenaline: Timer::idle(),
            adrenaline_share: 0.0,
            recovery_pool: 0.0,
            absorb: Absorb::none(),
            charge: None,
            war_stomp_armor: false,
            trance: TranceLimit::default(),
        }
    }

    /// **The maximum health multiplier**, from Last Stand.
    ///
    /// One place, read by the one function that pushes maximum health into the world, so a
    /// character sheet and a health bar cannot disagree about how big the Warrior is.
    #[must_use]
    pub fn max_health_multiplier(&self) -> f32 {
        let last_stand = if self.last_stand.is_active() {
            self.last_stand_bonus
        } else {
            0.0
        };
        1.0 + last_stand
    }

    /// **The armour multiplier**, from a full stack of War Stomp.
    #[must_use]
    pub fn armor_multiplier(&self) -> f32 {
        let stomp = if self.war_stomp_armor {
            WAR_STOMP_ARMOR_BONUS
        } else {
            0.0
        };
        1.0 + stomp
    }

    /// The extra Block chance Shield Block is granting, at `max_health`.
    ///
    /// Zero when it is not running, so the whole of "what is my Block right now" is one call.
    #[must_use]
    pub fn shield_block_bonus(&self, max_health: f32) -> f32 {
        if !self.shield_block.is_active() {
            return 0.0;
        }
        health_scaled(SHIELD_BLOCK_BASE, SHIELD_BLOCK_HEALTH_SCALE, max_health)
    }

    /// The flat damage reduction the Warrior currently has, as a fraction.
    #[must_use]
    pub fn damage_reduction(&self) -> f32 {
        if self.last_stand.is_active() {
            LAST_STAND_REDUCTION
        } else {
            0.0
        }
    }

    /// Whether critical strikes against the Warrior are currently turned into ordinary hits.
    #[must_use]
    pub fn immune_to_critical(&self) -> bool {
        self.shield_block.is_active()
    }

    /// Starts Battle Trance, and restarts its rate limit with it.
    pub fn start_battle_trance(&mut self, seconds: f32) {
        self.battle_trance.start(seconds);
        self.trance.restart();
    }

    /// Whether a defensive event may heal right now, spending one of the second's three
    /// activations if it may.
    pub fn take_trance_activation(&mut self) -> bool {
        if !self.battle_trance.is_active() {
            return false;
        }
        self.trance.take()
    }

    /// Banks `damage` if Adrenaline is running, and reports what went into the pool.
    pub fn bank_recovery(&mut self, damage: f32) -> f32 {
        if !self.adrenaline.is_active() || !damage.is_finite() || damage <= 0.0 {
            return 0.0;
        }
        let banked = damage * self.adrenaline_share;
        self.recovery_pool += banked;
        banked
    }

    /// Empties the recovery pool and hands back what was in it.
    pub fn take_recovery_pool(&mut self) -> f32 {
        let pool = self.recovery_pool;
        self.recovery_pool = 0.0;
        pool.max(0.0)
    }

    /// Advances every clock the Warrior owns.
    ///
    /// Returns whether Adrenaline ended on this call, which is the one expiry with a
    /// consequence the simulation has to act on.
    pub fn tick(&mut self, seconds: f32) -> bool {
        self.shield_block.tick(seconds);
        self.last_stand.tick(seconds);
        self.spell_reflect.tick(seconds);
        self.absorb.tick(seconds);
        if self.battle_trance.is_active() {
            self.trance.tick(seconds);
        }
        self.battle_trance.tick(seconds);
        self.adrenaline.tick(seconds)
    }

    /// Back to the start of a fight.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: f32 = 1.0 / 60.0;

    #[test]
    fn the_health_scaled_formula_hits_both_design_targets() {
        // No gear: the base number, exactly.
        let bare = health_scaled(
            SHIELD_BLOCK_BASE,
            SHIELD_BLOCK_HEALTH_SCALE,
            BASE_MAX_HEALTH,
        );
        assert!((bare - 0.06).abs() < 1e-6, "{bare}");

        // Fully geared: the design's +24 %.
        let geared = health_scaled(
            SHIELD_BLOCK_BASE,
            SHIELD_BLOCK_HEALTH_SCALE,
            BASE_MAX_HEALTH * GEARED_HEALTH_MULTIPLE,
        );
        assert!((geared - 0.24).abs() < 1e-4, "{geared}");

        // Last Stand is the same formula with its own two constants, and the same targets.
        let last_stand_bare =
            health_scaled(LAST_STAND_BASE, LAST_STAND_HEALTH_SCALE, BASE_MAX_HEALTH);
        let last_stand_geared = health_scaled(
            LAST_STAND_BASE,
            LAST_STAND_HEALTH_SCALE,
            BASE_MAX_HEALTH * GEARED_HEALTH_MULTIPLE,
        );
        assert!((last_stand_bare - 0.06).abs() < 1e-6);
        assert!((last_stand_geared - 0.24).abs() < 1e-4);
    }

    #[test]
    fn less_health_than_the_base_never_produces_a_negative_bonus() {
        let value = health_scaled(SHIELD_BLOCK_BASE, SHIELD_BLOCK_HEALTH_SCALE, 1.0);
        assert!((value - SHIELD_BLOCK_BASE).abs() < 1e-6, "{value}");
    }

    #[test]
    fn battle_trance_allows_three_activations_a_second_and_no_more() {
        let mut warrior = Warrior::new();
        warrior.start_battle_trance(10.0);
        // Ten attacks avoided in the same instant: three heal.
        let first = (0..10).filter(|_| warrior.take_trance_activation()).count();
        assert_eq!(first, 3);

        // A second later the allowance is back, and it is exactly three again.
        for _ in 0..60 {
            warrior.tick(STEP);
        }
        let second = (0..10).filter(|_| warrior.take_trance_activation()).count();
        assert_eq!(second, 3);
    }

    #[test]
    fn there_is_no_total_cap_on_battle_trance_healing() {
        let mut warrior = Warrior::new();
        warrior.start_battle_trance(10.0);
        let mut activations = 0;
        for _ in 0..10 {
            activations += (0..10).filter(|_| warrior.take_trance_activation()).count();
            for _ in 0..60 {
                warrior.tick(STEP);
            }
        }
        // Ten seconds, three a second: thirty, and nothing stopped it earlier.
        assert_eq!(activations, 30);
    }

    #[test]
    fn nothing_heals_when_battle_trance_is_not_running() {
        let mut warrior = Warrior::new();
        assert!(!warrior.take_trance_activation());
    }

    #[test]
    fn an_absorb_stands_in_front_of_health_until_it_is_used_up() {
        let mut absorb = Absorb::none();
        absorb.grant(300.0, 10.0);
        assert!(absorb.is_active());

        let (taken, through) = absorb.absorb(100.0);
        assert_eq!((taken, through), (100.0, 0.0));

        let (taken, through) = absorb.absorb(500.0);
        assert_eq!((taken, through), (200.0, 300.0));
        assert!(!absorb.is_active(), "an emptied shield is gone");
    }

    #[test]
    fn an_absorb_lapses_on_its_clock() {
        let mut absorb = Absorb::none();
        absorb.grant(300.0, 1.0);
        for _ in 0..90 {
            absorb.tick(STEP);
        }
        assert!(!absorb.is_active());
        let (taken, through) = absorb.absorb(100.0);
        assert_eq!((taken, through), (0.0, 100.0));
    }

    #[test]
    fn adrenaline_banks_only_while_it_is_running() {
        let mut warrior = Warrior::new();
        assert_eq!(warrior.bank_recovery(100.0), 0.0);

        warrior.adrenaline.start(6.0);
        warrior.adrenaline_share = ADRENALINE_SHARE;
        assert!((warrior.bank_recovery(100.0) - 30.0).abs() < 1e-4);
        assert!((warrior.bank_recovery(200.0) - 60.0).abs() < 1e-4);
        assert!((warrior.recovery_pool - 90.0).abs() < 1e-4);
        assert!((warrior.take_recovery_pool() - 90.0).abs() < 1e-4);
        assert_eq!(warrior.recovery_pool, 0.0);
    }

    #[test]
    fn the_multipliers_are_one_when_nothing_is_running() {
        let warrior = Warrior::new();
        assert_eq!(warrior.max_health_multiplier(), 1.0);
        assert_eq!(warrior.armor_multiplier(), 1.0);
        assert_eq!(warrior.damage_reduction(), 0.0);
        assert!(!warrior.immune_to_critical());
        assert_eq!(warrior.shield_block_bonus(BASE_MAX_HEALTH), 0.0);
    }
}
