//! FireMage: abilities, Ignite, Heat, talents and the fire on the ground.
//!
//! ```text
//! TalentGrid --> TalentEffects ----+
//!                                  |
//! abilities (rows) -> AbilityBook  |--> FireMage --> the simulation applies it
//!                                  |
//! IgniteTracker / ZoneField -------+
//! ```
//!
//! Every module here is state and rules with no side effects: an Ignite tracker reports that
//! a burn ticked, a zone reports that it burned, and the simulation turns those reports into
//! damage, events and deaths. That is what keeps one function responsible for every point of
//! damage in the game, and it is what makes all of this testable without a world.
//!
//! ## The three rules the class is built on
//!
//! **Reaching maximum Ignite does nothing.** No automatic explosion, no hidden timer, no
//! "primed" state. The payoff belongs to the player, who spends it with Detonate when it is
//! worth spending.
//!
//! **Nothing hardcodes five.** The Ignite maximum is
//! [`talent::TalentEffects::ignite_max_stacks`], read at the moment it is needed. Infernal
//! Capacity raises it to six and the Detonate requirement, the tooltips, the stack display
//! and every ability follow with no further change.
//!
//! **Nothing snapshots a stat.** [`stats::Stats`] is derived on demand from Heat, Fire
//! Mastery and the talents; burns and zones store coefficients and never numbers. A burn
//! applied at 10 % haste ticks at 30 % haste the instant the mage has it.

pub mod abilities;
pub mod beam;
pub mod ignite;
pub mod resource;
pub mod rng;
pub mod stats;
pub mod talent;
pub mod zone;

use engine_core::math::Vec3;

use crate::entity::EntityId;
use crate::hero::Hero;

// The shared ability types live in `crate::ability`; the FireMage's own rows are in
// `abilities`. Re-exported here because the FireMage owns the one ability clock
// (`FireMage::abilities`, `FireMage::global_cooldown`) every hero casts through.
pub use crate::ability::table::ABILITY_SLOTS;
pub use crate::ability::{
    ABILITIES, ABILITY_COUNT, AbilityBook, AbilityDef, AbilityError, AbilityState, ClassRow,
    DruidRow, FireAbility, FireMageRow, Motion, TARGET_CLEAVE_HALF_ANGLE, TargetMode, ThreatScale,
    ULTIMATE_ENABLED, WarriorRow,
};
pub use beam::{
    BEAM_COOLDOWN, BEAM_CORRECTION_SNAP, BEAM_DURATION, BEAM_ENERGY_COST, BEAM_IGNITE_EVERY_TICKS,
    BEAM_IGNITE_INTERVAL, BEAM_IGNITE_STACKS, BEAM_LENGTH, BEAM_MIN_AIM_DISTANCE,
    BEAM_MOVE_SPEED_MULTIPLIER, BEAM_REPORT_EPSILON, BEAM_REPORT_INTERVAL, BEAM_TICK_COEFFICIENT,
    BEAM_TICK_INTERVAL, BEAM_TICKS, BEAM_TURN_RATE, BEAM_TURN_RATE_DEGREES, BEAM_WIDTH,
    BeamAdvance, BeamChannel, BeamEnd, turn_towards, wrap_angle, yaw_to,
};
pub use ignite::{
    IGNITE_BASE_DURATION, IGNITE_BASE_MAX_STACKS, IGNITE_BASE_TICK_INTERVAL,
    IGNITE_SPELL_POWER_PER_STACK, Ignite, IgniteTick, IgniteTracker,
};
pub use resource::{
    ENERGY_MAXIMUM, ENERGY_PASSIVE_PER_SECOND, HEAT_COMBAT_TIMEOUT, HEAT_DECAY_PER_SECOND,
    HEAT_HOT_THRESHOLD, HEAT_MAXIMUM, HEAT_WARM_THRESHOLD, Heat, HeatTier, INFERNO_DURATION,
    UltimateEnergy,
};
pub use rng::Rng;
pub use stats::{
    BASE_CRIT_CHANCE, BASE_CRIT_DAMAGE, BASE_HASTE, BASE_SPELL_POWER, FireMastery, Gear, Stats,
};
pub use talent::{
    CRIT_FEEDBACK_CAP, DEFAULT_TALENT_BUDGET, TALENT_COUNT, TALENT_TIERS, TalentEffects,
    TalentError, TalentGrid, TalentId, total_talent_cost,
};
pub use zone::{PendingImpact, Zone, ZoneField, ZoneKind, ZoneShape, ZoneTick};

/// The global cooldown: how long after starting any ability before another may be started.
///
/// **One rule for every class.** Every hero's abilities start this one timer on the one
/// [`FireMage::global_cooldown`] clock, so the Warrior, the Druid and the FireMage cannot drift
/// apart and no ability row carries a number of its own. Haste shortens it exactly as it
/// shortens a cast ([`Stats::cast_time`]); at the base haste of zero it is exactly this.
pub const GLOBAL_COOLDOWN: f32 = 1.5;

/// Damage reduction while Flame Shield is up.
pub const FLAME_SHIELD_REDUCTION: f32 = 0.30;

/// A timer that counts down and reports whether it is running.
///
/// Every temporary effect on the class is one of these. Named rather than a bare `f32` so
/// "is Flame Shield up" cannot be written as `> 0.0` in one place and `>= 0.0` in another.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Timer {
    remaining: f32,
}

impl Timer {
    /// A timer that is not running.
    #[must_use]
    pub const fn idle() -> Self {
        Self { remaining: 0.0 }
    }

    /// Whether it is still running.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.remaining > 0.0
    }

    /// Seconds left.
    #[must_use]
    pub fn remaining(&self) -> f32 {
        self.remaining.max(0.0)
    }

    /// Starts it, keeping whichever duration is longer.
    pub fn start(&mut self, seconds: f32) {
        if seconds > self.remaining {
            self.remaining = seconds;
        }
    }

    /// Adds to a running timer.
    pub fn extend(&mut self, seconds: f32) {
        if self.remaining > 0.0 {
            self.remaining += seconds;
        }
    }

    /// Stops it.
    pub fn clear(&mut self) {
        self.remaining = 0.0;
    }

    /// Advances it, returning whether it expired on this call.
    pub fn tick(&mut self, seconds: f32) -> bool {
        if self.remaining <= 0.0 {
            return false;
        }
        self.remaining -= seconds;
        if self.remaining <= 0.0 {
            self.remaining = 0.0;
            return true;
        }
        false
    }
}

/// Which targets are standing in fire that makes them take more of it.
///
/// Slot-indexed like the burns, and for the same reason. A timer rather than a flag, so a
/// target that leaves a Molten Meteor's ground keeps the debuff for its remaining seconds
/// instead of shedding it on the frame it steps out.
#[derive(Debug, Default, Clone)]
pub struct VulnerabilityTracker {
    timers: Vec<f32>,
}

impl VulnerabilityTracker {
    /// An empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies or refreshes the debuff on `target`.
    pub fn apply(&mut self, target: EntityId, seconds: f32) {
        let slot = target.index();
        if self.timers.len() <= slot {
            self.timers.resize(slot + 1, 0.0);
        }
        self.timers[slot] = self.timers[slot].max(seconds);
    }

    /// Whether `target` currently takes extra fire damage.
    #[must_use]
    pub fn has(&self, target: EntityId) -> bool {
        self.timers
            .get(target.index())
            .is_some_and(|remaining| *remaining > 0.0)
    }

    /// Seconds of the debuff left on `target`. Zero when it has none.
    #[must_use]
    pub fn remaining(&self, target: EntityId) -> f32 {
        self.timers
            .get(target.index())
            .map_or(0.0, |remaining| remaining.max(0.0))
    }

    /// The extra fire damage multiplier `target` takes right now.
    #[must_use]
    pub fn multiplier(&self, target: EntityId, bonus: f32) -> f32 {
        if self.has(target) { 1.0 + bonus } else { 1.0 }
    }

    /// Advances every debuff.
    pub fn tick(&mut self, seconds: f32) {
        for remaining in &mut self.timers {
            if *remaining > 0.0 {
                *remaining = (*remaining - seconds).max(0.0);
            }
        }
    }

    /// Removes the debuff from one target.
    pub fn clear(&mut self, target: EntityId) {
        if let Some(remaining) = self.timers.get_mut(target.index()) {
            *remaining = 0.0;
        }
    }

    /// Removes every debuff.
    pub fn clear_all(&mut self) {
        self.timers.fill(0.0);
    }
}

/// Everything FireMage-specific about the player.
#[derive(Debug, Clone)]
pub struct FireMage {
    /// Cooldowns and charges.
    pub abilities: AbilityBook,
    /// Heat, its bands and the Inferno State.
    pub heat: Heat,
    /// Ultimate Energy: fills passively and from casting, and Moving Fire Beam spends all of it.
    pub energy: UltimateEnergy,
    /// Ignite on every target.
    pub ignite: IgniteTracker,
    /// Fire on the ground and rocks on their way to it.
    pub zones: ZoneField,
    /// The Ultimate's channel, while one is running.
    pub beam: BeamChannel,
    /// Who is taking extra fire damage from burning ground.
    pub vulnerability: VulnerabilityTracker,
    /// Which talents are bought.
    pub talents: TalentGrid,
    /// What those talents mean. Recomputed whenever the grid changes.
    effects: TalentEffects,
    /// Which stat Fire Mastery specialises into.
    pub mastery: FireMastery,
    /// What the character is wearing.
    ///
    /// Read by [`FireMage::stats`] like everything else, so changing gear mid-fight reaches
    /// a burn that is already running - the same rule the rest of the class follows.
    pub gear: Gear,
    /// The random source every chance in the class rolls against.
    pub rng: Rng,
    /// The global cooldown, started by any ability.
    pub global_cooldown: Timer,
    /// Fire Mastery's active buff.
    pub mastery_buff: Timer,
    /// Flame Shield's damage reduction.
    pub flame_shield: Timer,
}

impl Default for FireMage {
    fn default() -> Self {
        Self::new(0xF12E_5EED)
    }
}

impl FireMage {
    /// A fresh FireMage with the starting talent points.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let talents = TalentGrid::new(Hero::FireMage);
        let effects = TalentEffects::from_grid(&talents);
        Self {
            abilities: AbilityBook::new(&effects),
            heat: Heat::new(),
            energy: UltimateEnergy::new(),
            ignite: IgniteTracker::new(),
            zones: ZoneField::new(),
            beam: BeamChannel::idle(),
            vulnerability: VulnerabilityTracker::new(),
            talents,
            effects,
            mastery: FireMastery::default(),
            gear: Gear::none(),
            rng: Rng::new(seed),
            global_cooldown: Timer::idle(),
            mastery_buff: Timer::idle(),
            flame_shield: Timer::idle(),
        }
    }

    /// What the talents currently mean.
    #[must_use]
    pub fn effects(&self) -> &TalentEffects {
        &self.effects
    }

    /// **The stats of this moment.** Everything that decides a number reads this.
    ///
    /// Derived, never stored: Heat's band, Fire Mastery's active and the talents are added
    /// up on every call. A burn that is already running sees the result of this function on
    /// its next tick, which is what "no snapshot" means in practice.
    #[must_use]
    pub fn stats(&self) -> Stats {
        let mut stats = Stats::base();
        stats.equip(&self.gear);
        let dual = self.effects.dual_mastery;

        // Heat's band. Dual Fire Mastery splits the threshold grant across both stats
        // rather than doubling it.
        let banded = self.heat.stat_bonus();
        if banded > 0.0 {
            let amount = if dual { banded * 0.5 } else { banded };
            stats.grant(self.mastery, amount, dual);
        }

        // Fire Mastery's active.
        if self.mastery_buff.is_active() {
            stats.grant(self.mastery, self.effects.mastery_active_bonus, dual);
        }

        stats
    }

    /// The current haste, which shortens casts, cooldowns and Ignite's interval.
    #[must_use]
    pub fn haste(&self) -> f32 {
        self.stats().haste
    }

    /// The chance a fire spell crits right now.
    #[must_use]
    pub fn crit_chance(&self) -> f32 {
        self.stats().crit()
    }

    /// Ignite's tick interval right now, haste included.
    #[must_use]
    pub fn ignite_interval(&self) -> f32 {
        self.stats().interval(IGNITE_BASE_TICK_INTERVAL)
    }

    /// The current maximum Ignite stacks. The number everything else asks for.
    #[must_use]
    pub fn ignite_max(&self) -> u8 {
        self.effects.ignite_max_stacks
    }

    /// Ignite's current duration, talents included.
    #[must_use]
    pub fn ignite_duration(&self) -> f32 {
        self.effects.ignite_duration()
    }

    /// What is left of a burn on `target`, in damage, at the stats of this moment.
    ///
    /// **What Detonate converts.** Expressed as damage rather than as a coefficient so the
    /// simulator, the tooltip and the explosion cannot disagree about it.
    #[must_use]
    pub fn remaining_ignite_damage(&self, target: EntityId) -> f32 {
        let stats = self.stats();
        let burn = self.ignite.get(target);
        burn.remaining_coefficient(
            stats.interval(IGNITE_BASE_TICK_INTERVAL),
            self.effects.ignite_damage_scale,
        ) * stats.spell_power
            * self.heat.damage_multiplier()
    }

    /// Buys a talent and rebuilds the derived effects.
    ///
    /// # Errors
    ///
    /// Returns [`TalentError`] if it is owned already or cannot be afforded.
    pub fn purchase_talent(&mut self, talent: TalentId) -> Result<(), TalentError> {
        self.talents.purchase(talent)?;
        self.refresh_talents();
        Ok(())
    }

    /// Replaces the whole build. What the simulator and a respec do.
    pub fn set_talents(&mut self, talents: TalentGrid) {
        self.talents = talents;
        self.refresh_talents();
        self.abilities.refill(&self.effects);
    }

    /// Recomputes the derived effects and lets the ability book grow into new charges.
    pub fn refresh_talents(&mut self) {
        let before = self.effects;
        self.effects = TalentEffects::from_grid(&self.talents);
        for ability in FireAbility::ALL {
            let was = ability.definition().charges_with(&before);
            let now = ability.definition().charges_with(&self.effects);
            if now > was && self.abilities.charges(ability) == was {
                self.abilities.grant_charge(ability, &self.effects);
            }
        }
    }

    /// Whether an ability may be started right now.
    #[must_use]
    pub fn off_global_cooldown(&self) -> bool {
        !self.global_cooldown.is_active()
    }

    /// How far through the global cooldown we are, in `0.0..=1.0`. One means ready.
    #[must_use]
    pub fn global_cooldown_fraction(&self) -> f32 {
        let total = self.global_cooldown_duration();
        if total <= 0.0 || !self.global_cooldown.is_active() {
            return 1.0;
        }
        (1.0 - self.global_cooldown.remaining() / total).clamp(0.0, 1.0)
    }

    /// How long the global cooldown lasts right now, haste included.
    #[must_use]
    pub fn global_cooldown_duration(&self) -> f32 {
        self.stats().cast_time(GLOBAL_COOLDOWN)
    }

    /// Seconds until the global cooldown ends. Zero when it is not running.
    #[must_use]
    pub fn global_cooldown_remaining(&self) -> f32 {
        self.global_cooldown.remaining()
    }

    /// Starts the global cooldown. Called when an ability is started, not when it lands.
    pub fn start_global_cooldown(&mut self) {
        let duration = self.global_cooldown_duration();
        self.global_cooldown.start(duration);
    }

    /// **Whether the global cooldown holds `ability` back right now.**
    ///
    /// The one place the per-ability [`AbilityDef::on_global_cooldown`] column is read: an
    /// ability off the global cooldown is never held, whatever the timer says. The cast
    /// path, the input layer and the action bar all ask this rather than each combining the
    /// column with the timer themselves.
    ///
    /// [`AbilityDef::on_global_cooldown`]: crate::ability::table::AbilityDef::on_global_cooldown
    #[must_use]
    pub fn global_cooldown_holds(&self, ability: FireAbility) -> bool {
        ability.definition().on_global_cooldown && !self.off_global_cooldown()
    }

    /// [`FireMage::global_cooldown_fraction`] as `ability`'s button sees it: always one for
    /// an ability the global cooldown does not hold, so it never draws the shared sweep.
    #[must_use]
    pub fn global_cooldown_fraction_for(&self, ability: FireAbility) -> f32 {
        if ability.definition().on_global_cooldown {
            self.global_cooldown_fraction()
        } else {
            1.0
        }
    }

    /// Starts the global cooldown for a press of `ability` — or leaves it exactly as it is,
    /// for an ability that is off it.
    pub fn start_global_cooldown_for(&mut self, ability: FireAbility) {
        if ability.definition().on_global_cooldown {
            self.start_global_cooldown();
        }
    }

    /// Ends the global cooldown early. What a cast that did not complete gives back.
    pub fn clear_global_cooldown(&mut self) {
        self.global_cooldown.clear();
    }

    /// Rolls a critical strike against the current chance.
    pub fn roll_crit(&mut self) -> bool {
        let chance = self.crit_chance();
        self.rng.chance(chance)
    }

    /// Every multiplier that applies to a point of fire damage right now.
    ///
    /// One function so a new source of damage cannot forget one of them, and so the tooltip
    /// and the damage agree about what the class currently hits for.
    #[must_use]
    pub fn fire_damage_multiplier(&self) -> f32 {
        self.heat.damage_multiplier()
    }

    /// Adds Heat, starting the Inferno State if this reached the top of the bar.
    ///
    /// Returns whether the state started, which is what the interface flashes on.
    pub fn gain_heat(&mut self, amount: f32) -> bool {
        let duration = self.effects.inferno_duration();
        self.heat.gain(amount, duration)
    }

    /// Notes that the mage is in combat, which holds Heat's decay off.
    pub fn note_combat(&mut self) {
        self.heat.note_combat();
    }

    /// Adds Ultimate Energy.
    pub fn gain_energy(&mut self, amount: f32) {
        self.energy.gain(amount);
    }

    /// The damage `amount` becomes after Flame Shield.
    #[must_use]
    pub fn incoming_damage(&self, amount: f32) -> f32 {
        if self.flame_shield.is_active() {
            return amount * (1.0 - FLAME_SHIELD_REDUCTION);
        }
        amount
    }

    /// Advances every timer, the resources and the ability cooldowns.
    ///
    /// Zones and Ignite are advanced by the simulation, which needs their reports to deal
    /// damage; everything without a side effect happens here.
    pub fn tick(&mut self, seconds: f32) {
        let haste = self.haste();
        self.abilities.tick(seconds, haste, &self.effects);
        self.heat.tick(seconds);
        self.energy.tick(seconds);
        self.vulnerability.tick(seconds);
        self.global_cooldown.tick(seconds);
        self.mastery_buff.tick(seconds);
        self.flame_shield.tick(seconds);
    }

    /// Back to the start of a fight, keeping the talents and the specialisation.
    pub fn reset(&mut self) {
        self.abilities.refill(&self.effects);
        self.heat.clear();
        self.energy.clear();
        self.ignite.clear_all();
        self.zones.clear();
        self.beam.stop();
        self.vulnerability.clear_all();
        self.global_cooldown.clear();
        self.mastery_buff.clear();
        self.flame_shield.clear();
    }

    /// Moves nothing: zones stay where they were placed.
    ///
    /// Kept as a no-op hook because the simulation calls it every tick and a future
    /// caster-following zone would want it.
    pub fn follow(&mut self, _position: Vec3) {}
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_firemage_starts_cold_and_ready() {
        let mage = FireMage::new(1);
        assert_eq!(mage.heat.value(), 0.0);
        assert_eq!(mage.ignite_max(), IGNITE_BASE_MAX_STACKS);
        assert_eq!(mage.ignite_duration(), IGNITE_BASE_DURATION);
        for ability in FireAbility::ALL {
            assert!(mage.abilities.is_ready(ability), "{ability:?} is not ready");
        }
    }

    #[test]
    fn buying_infernal_capacity_raises_the_maximum_everywhere() {
        let mut mage = FireMage::new(1);
        assert_eq!(mage.ignite_max(), 5);
        mage.talents.grant_points(100);
        mage.purchase_talent(TalentId::InfernalCapacity)
            .expect("testing mode pays for it");
        assert_eq!(mage.ignite_max(), 6);

        let target = crate::entity::EntityId::new(0, 1);
        for _ in 0..10 {
            mage.ignite
                .apply(target, 1, mage.ignite_max(), mage.ignite_duration());
        }
        assert_eq!(mage.ignite.stacks(target), 6);
    }

    #[test]
    fn heat_bands_raise_damage_and_then_hand_over_a_stat() {
        let mut mage = FireMage::new(1);
        assert!((mage.fire_damage_multiplier() - 1.0).abs() < 1e-6);
        assert!((mage.haste() - BASE_HASTE).abs() < 1e-6);

        mage.gain_heat(30.0);
        assert!((mage.fire_damage_multiplier() - 1.15).abs() < 1e-6);
        assert!((mage.haste() - BASE_HASTE).abs() < 1e-6, "warm gave a stat");

        mage.gain_heat(40.0);
        assert!((mage.fire_damage_multiplier() - 1.30).abs() < 1e-6);
        assert!((mage.haste() - 0.05).abs() < 1e-6, "hot gave no haste");

        // Crit mastery puts the same grant in the other stat.
        mage.mastery = FireMastery::Crit;
        assert!((mage.haste() - BASE_HASTE).abs() < 1e-6);
        assert!((mage.crit_chance() - (BASE_CRIT_CHANCE + 0.05)).abs() < 1e-6);
    }

    #[test]
    fn the_inferno_state_is_the_top_of_the_bar_and_ends_at_zero() {
        let mut mage = FireMage::new(1);
        assert!(mage.gain_heat(100.0), "the state did not start");
        assert_eq!(mage.heat.value(), 0.0);
        assert!((mage.fire_damage_multiplier() - 1.50).abs() < 1e-6);
        assert!((mage.haste() - 0.075).abs() < 1e-6);

        mage.tick(INFERNO_DURATION + 0.1);
        assert!(!mage.heat.is_inferno());
        assert_eq!(mage.heat.value(), 0.0);
        assert!((mage.fire_damage_multiplier() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn furnace_mastery_lengthens_the_state_through_the_talent_effects() {
        let mut mage = FireMage::new(1);
        mage.talents.grant_points(100);
        mage.purchase_talent(TalentId::FurnaceMastery)
            .expect("affordable");
        mage.gain_heat(100.0);
        assert!((mage.heat.inferno_remaining() - 12.0).abs() < 1e-5);
    }

    #[test]
    fn fire_mastery_grants_its_stat_and_expires() {
        let mut mage = FireMage::new(1);
        mage.mastery_buff.start(20.0);
        assert!((mage.haste() - 0.15).abs() < 1e-6);
        assert!((mage.crit_chance() - BASE_CRIT_CHANCE).abs() < 1e-6);

        mage.tick(20.1);
        assert!((mage.haste() - BASE_HASTE).abs() < 1e-6);
    }

    #[test]
    fn dual_fire_mastery_splits_the_grant_across_both_stats() {
        let mut mage = FireMage::new(1);
        mage.talents.grant_points(100);
        mage.purchase_talent(TalentId::DualFireMastery)
            .expect("affordable");
        mage.mastery_buff.start(20.0);
        assert!((mage.haste() - 0.10).abs() < 1e-6);
        assert!((mage.crit_chance() - (BASE_CRIT_CHANCE + 0.10)).abs() < 1e-6);

        // And the threshold grant is halved rather than doubled.
        mage.gain_heat(70.0);
        assert!((mage.haste() - 0.125).abs() < 1e-6, "{}", mage.haste());
    }

    #[test]
    fn ignites_interval_follows_haste_with_no_reapplication() {
        let mut mage = FireMage::new(1);
        assert!((mage.ignite_interval() - IGNITE_BASE_TICK_INTERVAL).abs() < 1e-6);

        let target = crate::entity::EntityId::new(1, 1);
        mage.ignite
            .apply(target, 3, mage.ignite_max(), mage.ignite_duration());

        mage.mastery_buff.start(20.0);
        let hasted = mage.ignite_interval();
        assert!(hasted < IGNITE_BASE_TICK_INTERVAL, "{hasted}");
        assert!((hasted - IGNITE_BASE_TICK_INTERVAL / 1.15).abs() < 1e-5);
    }

    #[test]
    fn a_burns_remaining_damage_follows_the_stats_of_the_moment() {
        // No snapshot, at the level the explosion reads it: the same burn is worth more
        // once Heat has moved, with no reapplication.
        let mut mage = FireMage::new(1);
        let target = crate::entity::EntityId::new(2, 1);
        mage.ignite
            .apply(target, 5, mage.ignite_max(), mage.ignite_duration());

        let cold = mage.remaining_ignite_damage(target);
        assert!(cold > 0.0);

        mage.gain_heat(35.0);
        let warm = mage.remaining_ignite_damage(target);
        assert!((warm / cold - 1.15).abs() < 1e-4, "{warm} vs {cold}");
    }

    #[test]
    fn volatile_ignition_weakens_the_burn_it_is_paid_for_with() {
        let mut mage = FireMage::new(1);
        let target = crate::entity::EntityId::new(3, 1);
        mage.ignite
            .apply(target, 5, mage.ignite_max(), mage.ignite_duration());
        let full = mage.remaining_ignite_damage(target);

        mage.talents.grant_points(100);
        mage.purchase_talent(TalentId::VolatileIgnition)
            .expect("affordable");
        let weakened = mage.remaining_ignite_damage(target);
        assert!((weakened / full - 0.80).abs() < 1e-4);
    }

    #[test]
    fn heat_holds_in_combat_and_drains_once_the_fight_is_over() {
        let mut mage = FireMage::new(1);
        mage.gain_heat(50.0);
        for _ in 0..100 {
            mage.note_combat();
            mage.tick(0.1);
        }
        assert!((mage.heat.value() - 50.0).abs() < 1e-3);

        mage.tick(HEAT_COMBAT_TIMEOUT + 5.0);
        assert!(mage.heat.value() < 50.0, "it never decayed");
    }

    #[test]
    fn the_vulnerability_debuff_outlives_the_ground_by_its_own_timer() {
        let mut mage = FireMage::new(1);
        let target = crate::entity::EntityId::new(4, 1);
        assert!(!mage.vulnerability.has(target));

        let duration = mage.effects().vulnerability_duration();
        mage.vulnerability.apply(target, duration);
        assert!(mage.vulnerability.has(target));
        assert!(
            (mage
                .vulnerability
                .multiplier(target, mage.effects().meteor_ground_vulnerability)
                - 1.03)
                .abs()
                < 1e-6
        );

        mage.tick(duration + 0.1);
        assert!(!mage.vulnerability.has(target));
    }

    #[test]
    fn an_ability_off_the_global_cooldown_is_never_held_by_it_and_never_starts_it() {
        let mut mage = FireMage::new(1);
        mage.start_global_cooldown_for(FireAbility::Blink);
        assert!(
            mage.off_global_cooldown(),
            "Blink started the global cooldown"
        );

        mage.start_global_cooldown_for(FireAbility::Firebolt);
        assert!(mage.global_cooldown_holds(FireAbility::Firebolt));
        assert!(mage.global_cooldown_fraction_for(FireAbility::Firebolt) < 1.0);
        for ability in [
            FireAbility::Blink,
            FireAbility::Detonate,
            FireAbility::FireMastery,
        ] {
            assert!(!mage.global_cooldown_holds(ability), "{ability:?}");
            assert_eq!(mage.global_cooldown_fraction_for(ability), 1.0);
        }
    }

    #[test]
    fn the_global_cooldown_runs_for_a_second_and_a_half_and_is_shortened_by_haste() {
        let mut mage = FireMage::new(1);
        assert!(mage.off_global_cooldown());
        assert!((GLOBAL_COOLDOWN - 1.5).abs() < f32::EPSILON);
        assert!((mage.global_cooldown_duration() - 1.5).abs() < 1e-6);

        mage.start_global_cooldown();
        assert!(!mage.off_global_cooldown());
        mage.tick(1.4);
        assert!(
            !mage.off_global_cooldown(),
            "it ended before a second and a half"
        );
        mage.tick(0.11);
        assert!(mage.off_global_cooldown());

        mage.mastery_buff.start(20.0);
        assert!(mage.global_cooldown_duration() < GLOBAL_COOLDOWN);
    }

    #[test]
    fn resetting_returns_a_fight_to_its_start_but_keeps_the_build() {
        let mut mage = FireMage::new(1);
        mage.talents.grant_points(100);
        mage.purchase_talent(TalentId::InfernalCapacity)
            .expect("affordable");
        mage.mastery = FireMastery::Crit;
        mage.gain_heat(50.0);
        let effects = *mage.effects();
        mage.abilities.spend(FireAbility::Meteor, &effects);

        mage.reset();
        assert_eq!(mage.heat.value(), 0.0);
        assert!(mage.abilities.is_ready(FireAbility::Meteor));
        assert_eq!(mage.ignite_max(), 6, "the talent was refunded");
        assert_eq!(mage.mastery, FireMastery::Crit);
    }

    #[test]
    fn a_timer_keeps_the_longer_of_two_durations() {
        let mut timer = Timer::idle();
        timer.start(5.0);
        timer.tick(2.0);
        timer.start(1.0);
        assert!((timer.remaining() - 3.0).abs() < 1e-6);
        timer.start(10.0);
        assert!((timer.remaining() - 10.0).abs() < 1e-6);
    }
}
