//! FireMage: abilities, Ignite, Heat, Ultimate Energy, talents and the fire on the ground.
//!
//! ```text
//! TalentGrid --> TalentEffects ----+
//!                                  |
//! ABILITIES (data) --> AbilityBook |--> FireMage --> the simulation applies it
//!                                  |
//! IgniteTracker / ZoneField -------+
//! ```
//!
//! Every module here is state and rules with no side effects: an Ignite tracker reports that
//! a burn ticked, a zone reports that it burned, and the simulation turns those reports into
//! damage, events and deaths. That is what keeps one function responsible for every point of
//! damage in the game, and it is what makes all of this testable without a world.
//!
//! ## The one rule the class is built on
//!
//! Reaching maximum Ignite stacks does nothing. No automatic explosion, no hidden timer. The
//! payoff belongs to the player, who spends it with Fire Detonation when it is worth
//! spending. Every ability that applies Ignite - Firebolt, Flame Wave, Meteor, Firestorm,
//! Inferno, Cataclysm - stops at the cap and waits.
//!
//! ## Nothing hardcodes five
//!
//! The Ignite maximum is [`talent::TalentEffects::ignite_max_stacks`], read from the talents
//! at the moment it is needed. Infernal Capacity raises it to six and the detonation
//! requirement, the tooltips, the stack display and every ability follow with no further
//! change.

pub mod ability;
pub mod ignite;
pub mod resource;
pub mod rng;
pub mod talent;
pub mod zone;

use engine_core::math::Vec3;

pub use ability::{
    ABILITIES, ABILITY_COUNT, ABILITY_SLOTS, AbilityBook, AbilityDef, AbilityError, AbilityState,
    BASE_CRIT_CHANCE, CRIT_MULTIPLIER, FireAbility, TargetMode,
};
pub use ignite::{
    IGNITE_BASE_DURATION, IGNITE_BASE_MAX_STACKS, IGNITE_DAMAGE_PER_STACK_PER_SECOND, Ignite,
    IgniteTick, IgniteTracker,
};
pub use resource::{
    ENERGY_MAXIMUM, ENERGY_PASSIVE_PER_SECOND, HEAT_HIGH_THRESHOLD, HEAT_MAXIMUM,
    HEAT_OVERHEAT_THRESHOLD, Heat, UltimateEnergy,
};
pub use rng::Rng;
pub use talent::{
    TALENT_COUNT, TALENT_TIERS, TALENTS, TESTING_MODE, TalentDef, TalentEffects, TalentError,
    TalentGrid, TalentId, starting_talent_points, total_talent_cost,
};
pub use zone::{PendingImpact, Zone, ZoneField, ZoneKind, ZoneTick};

/// The global cooldown: how long after starting any ability before another may be started.
///
/// Half a second, the genre's standard. It is what stops two instants from going off in the
/// same frame, and it is deliberately *shorter than most cast times* - a one-second Firebolt
/// outlasts it, so the next cast can begin the moment the last one lands and the rotation is
/// limited by casting rather than by a timer.
pub const GLOBAL_COOLDOWN: f32 = 0.5;

/// Damage reduction while Flame Shield is up.
pub const FLAME_SHIELD_REDUCTION: f32 = 0.30;

/// Critical strike bonus while Inferno is next to a fully burning enemy.
pub const INFERNO_CRIT_BONUS: f32 = 0.20;

/// How long Inferno's critical strike bonus lasts once granted, in seconds.
pub const INFERNO_CRIT_DURATION: f32 = 4.0;

/// How long Living Inferno's haste lasts, in seconds.
pub const LIVING_INFERNO_DURATION: f32 = 5.0;

/// Pyroclasm's internal cooldown, in seconds.
pub const PYROCLASM_INTERNAL_COOLDOWN: f32 = 10.0;

/// How long Phoenix Protocol's invulnerability lasts, in seconds.
pub const PHOENIX_INVULNERABILITY: f32 = 2.0;

/// Phoenix Protocol's cooldown, in seconds.
pub const PHOENIX_COOLDOWN: f32 = 180.0;

/// How much Cataclysm's infernal ground slows what stands in it.
pub const INFERNAL_GROUND_SLOW: f32 = 0.30;

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
    ///
    /// Longer rather than newest: refreshing a five-second shield two seconds in should not
    /// shorten it to what is left of a shorter one.
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

/// Everything FireMage-specific about the player.
///
/// The simulation owns one of these. It holds no entity, no world and no events: it is the
/// class's state, and the simulation is what connects it to a fight.
#[derive(Debug, Clone)]
pub struct FireMage {
    /// Cooldowns and charges.
    pub abilities: AbilityBook,
    /// Heat, and whether it has boiled over.
    pub heat: Heat,
    /// Ultimate Energy.
    pub energy: UltimateEnergy,
    /// Ignite on every target.
    pub ignite: IgniteTracker,
    /// Fire on the ground and rocks on their way to it.
    pub zones: ZoneField,
    /// Which talents are bought.
    pub talents: TalentGrid,
    /// What those talents mean. Recomputed whenever the grid changes.
    effects: TalentEffects,
    /// The random source every chance in the class rolls against.
    pub rng: Rng,
    /// The global cooldown, started by any ability.
    pub global_cooldown: Timer,
    /// Flame Shield's damage reduction.
    pub flame_shield: Timer,
    /// Inferno's critical strike bonus.
    pub inferno_crit: Timer,
    /// Living Inferno's haste.
    pub haste_buff: Timer,
    /// Phoenix Protocol's invulnerability.
    pub invulnerable: Timer,
    /// Phoenix Protocol's cooldown.
    pub phoenix_cooldown: Timer,
    /// Pyroclasm's internal cooldown.
    pub pyroclasm_cooldown: Timer,
    /// Whether the next fire spell is empowered by Volatile Heat.
    pub volatile_heat_ready: bool,
    /// Whether the next fire spell is guaranteed to crit, from Pyroclasm.
    pub guaranteed_crit: bool,
    /// Whether Cataclysm's ground is burning, which Cataclysmic Ignition reads.
    pub cataclysm_active: bool,
    /// Seconds of Inferno extension already granted this cast.
    pub inferno_extension: f32,
    /// Heat generated by the running Firestorm, against its per-cast cap.
    pub firestorm_heat: f32,
    /// Heat generated by the running Cataclysm ground, against its cap.
    pub zone_heat: f32,
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
        let talents = TalentGrid::new();
        let effects = talents.effects();
        Self {
            abilities: AbilityBook::new(&effects),
            heat: Heat::new(),
            energy: UltimateEnergy::new(),
            ignite: IgniteTracker::new(),
            zones: ZoneField::new(),
            talents,
            effects,
            rng: Rng::new(seed),
            global_cooldown: Timer::idle(),
            flame_shield: Timer::idle(),
            inferno_crit: Timer::idle(),
            haste_buff: Timer::idle(),
            invulnerable: Timer::idle(),
            phoenix_cooldown: Timer::idle(),
            pyroclasm_cooldown: Timer::idle(),
            volatile_heat_ready: false,
            guaranteed_crit: false,
            cataclysm_active: false,
            inferno_extension: 0.0,
            firestorm_heat: 0.0,
            zone_heat: 0.0,
        }
    }

    /// What the talents currently mean.
    #[must_use]
    pub fn effects(&self) -> &TalentEffects {
        &self.effects
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

    /// Recomputes the derived effects and lets the ability book grow into new charges.
    ///
    /// Called after any change to the grid. A talent that adds a charge hands it over
    /// immediately: the player just paid for it, and making them wait out a cooldown they
    /// were not on would be a strange way to sell an upgrade.
    pub fn refresh_talents(&mut self) {
        let before = self.effects;
        self.effects = self.talents.effects();
        for ability in FireAbility::ALL {
            let was = ability.definition().charges_with(&before);
            let now = ability.definition().charges_with(&self.effects);
            if now > was && self.abilities.charges(ability) == was {
                // Reducing by the whole cooldown is how a charge is granted; the book
                // clamps at the new maximum on its own.
                self.abilities
                    .reduce_cooldown(ability, f32::MAX, &self.effects);
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
    ///
    /// What the bar draws its sweep from, computed here so the interface cannot disagree
    /// with the rule that refuses the cast.
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
        GLOBAL_COOLDOWN / (1.0 + self.haste())
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

    /// Ends the global cooldown early.
    ///
    /// What a cast that did not complete gives back. The timer is started when a cast
    /// *begins*, so a cancelled cast would otherwise leave the whole bar locked for the
    /// remainder of a cooldown that no ability ever paid for — the same reasoning that
    /// refunds the charge and the Ultimate's energy.
    pub fn clear_global_cooldown(&mut self) {
        self.global_cooldown.clear();
    }

    /// The current haste, which shortens casts and cooldowns.
    #[must_use]
    pub fn haste(&self) -> f32 {
        if self.haste_buff.is_active() {
            self.effects.living_inferno_haste
        } else {
            0.0
        }
    }

    /// The chance a fire spell crits right now.
    #[must_use]
    pub fn crit_chance(&self) -> f32 {
        let bonus = if self.inferno_crit.is_active() {
            INFERNO_CRIT_BONUS
        } else {
            0.0
        };
        (BASE_CRIT_CHANCE + bonus).clamp(0.0, 1.0)
    }

    /// Rolls a critical strike, honouring a guaranteed one.
    ///
    /// Consumes the guarantee: Pyroclasm empowers the *next* spell, not every spell until
    /// the next Pyroblast.
    pub fn roll_crit(&mut self) -> bool {
        if self.guaranteed_crit {
            self.guaranteed_crit = false;
            return true;
        }
        let chance = self.crit_chance();
        self.rng.chance(chance)
    }

    /// Every multiplier that applies to a point of fire damage right now.
    ///
    /// One function so a new source of damage cannot forget one of them, and so the tooltip
    /// and the damage agree about what the class currently hits for.
    #[must_use]
    pub fn fire_damage_multiplier(&self) -> f32 {
        let mut multiplier = self.heat.damage_multiplier();
        multiplier *= 1.0 + self.effects.arcane_pressure_bonus(self.energy.value());
        multiplier *= 1.0 + self.effects.furnace_overdrive_bonus(self.heat.value());
        if self.volatile_heat_ready {
            multiplier *= 1.0 + self.effects.volatile_heat_bonus;
        }
        multiplier
    }

    /// Consumes the one-shot damage bonuses a spell just used.
    pub fn consume_spell_buffs(&mut self) {
        self.volatile_heat_ready = false;
    }

    /// Adds Heat, applying everything that happens on boiling over.
    pub fn gain_heat(&mut self, amount: f32) {
        if self.heat.gain(amount) {
            if self.effects.volatile_heat_bonus > 0.0 {
                self.volatile_heat_ready = true;
            }
            if self.effects.living_inferno_haste > 0.0 {
                self.haste_buff.start(LIVING_INFERNO_DURATION);
            }
        }
    }

    /// Adds Ultimate Energy.
    pub fn gain_energy(&mut self, amount: f32) {
        self.energy.gain(amount);
    }

    /// The damage `amount` becomes after Flame Shield and invulnerability.
    #[must_use]
    pub fn incoming_damage(&self, amount: f32) -> f32 {
        if self.invulnerable.is_active() {
            return 0.0;
        }
        if self.flame_shield.is_active() {
            return amount * (1.0 - FLAME_SHIELD_REDUCTION);
        }
        amount
    }

    /// Whether a lethal hit should be survived, spending Phoenix Protocol if so.
    ///
    /// Called by the simulation before damage lands. Returns whether the hit was cheated.
    pub fn try_survive_lethal(&mut self) -> bool {
        if !self.effects.phoenix_protocol || self.phoenix_cooldown.is_active() {
            return false;
        }
        self.phoenix_cooldown.start(PHOENIX_COOLDOWN);
        self.invulnerable.start(PHOENIX_INVULNERABILITY);
        true
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
        self.global_cooldown.tick(seconds);
        self.flame_shield.tick(seconds);
        self.haste_buff.tick(seconds);
        self.invulnerable.tick(seconds);
        self.phoenix_cooldown.tick(seconds);
        self.pyroclasm_cooldown.tick(seconds);
        if self.inferno_crit.tick(seconds) {
            self.inferno_extension = 0.0;
        }
        if !self.zones.has(ZoneKind::Firestorm) {
            self.firestorm_heat = 0.0;
        }
        if !self.zones.has(ZoneKind::InfernalGround) {
            self.cataclysm_active = false;
            self.zone_heat = 0.0;
        }
    }

    /// Moves the caster-following zones with the player.
    pub fn follow(&mut self, position: Vec3) {
        self.zones.follow(position);
    }

    /// Back to the start of a fight, keeping the talents.
    pub fn reset(&mut self) {
        self.abilities.refill(&self.effects);
        self.heat.clear();
        self.energy.clear();
        self.ignite.clear_all();
        self.zones.clear();
        self.global_cooldown.clear();
        self.flame_shield.clear();
        self.inferno_crit.clear();
        self.haste_buff.clear();
        self.invulnerable.clear();
        self.phoenix_cooldown.clear();
        self.pyroclasm_cooldown.clear();
        self.volatile_heat_ready = false;
        self.guaranteed_crit = false;
        self.cataclysm_active = false;
        self.inferno_extension = 0.0;
        self.firestorm_heat = 0.0;
        self.zone_heat = 0.0;
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_firemage_starts_cold_empty_and_ready() {
        let mage = FireMage::new(1);
        assert_eq!(mage.heat.value(), 0.0);
        assert_eq!(mage.energy.value(), 0.0);
        assert_eq!(mage.ignite_max(), IGNITE_BASE_MAX_STACKS);
        for ability in FireAbility::ALL {
            assert!(mage.abilities.is_ready(ability), "{ability:?} is not ready");
        }
    }

    #[test]
    fn testing_mode_hands_over_the_whole_tree() {
        let mage = FireMage::new(1);
        if TESTING_MODE {
            assert_eq!(mage.talents.available_points(), total_talent_cost());
        } else {
            assert_eq!(mage.talents.available_points(), 0);
        }
    }

    #[test]
    fn buying_infernal_capacity_raises_the_maximum_everywhere() {
        let mut mage = FireMage::new(1);
        assert_eq!(mage.ignite_max(), 5);
        mage.purchase_talent(TalentId::InfernalCapacity)
            .expect("testing mode pays for it");
        assert_eq!(mage.ignite_max(), 6);
        assert_eq!(mage.effects().ignite_max_stacks, 6);

        // And the tracker follows, because the maximum is passed in rather than baked in.
        let target = crate::entity::EntityId::new(0, 1);
        for _ in 0..10 {
            mage.ignite
                .apply(target, 1, mage.ignite_max(), mage.ignite_duration());
        }
        assert_eq!(mage.ignite.stacks(target), 6);
    }

    #[test]
    fn buying_meteor_shower_hands_over_the_extra_charge_at_once() {
        let mut mage = FireMage::new(1);
        assert_eq!(mage.abilities.charges(FireAbility::Meteor), 1);
        mage.purchase_talent(TalentId::MeteorShower)
            .expect("testing mode pays for it");
        assert_eq!(
            FireAbility::Meteor
                .definition()
                .charges_with(mage.effects()),
            2
        );
    }

    #[test]
    fn heat_boiling_over_arms_the_talents_that_watch_for_it() {
        let mut mage = FireMage::new(1);
        mage.purchase_talent(TalentId::VolatileHeat)
            .expect("affordable");
        mage.purchase_talent(TalentId::LivingInferno)
            .expect("affordable");

        mage.gain_heat(99.0);
        assert!(!mage.volatile_heat_ready);
        assert!(!mage.haste_buff.is_active());

        mage.gain_heat(1.0);
        assert!(mage.volatile_heat_ready, "volatile heat did not arm");
        assert!(mage.haste_buff.is_active(), "living inferno did not fire");
        assert!(mage.haste() > 0.0);

        // And the empowerment is spent by one spell, not held forever.
        let empowered = mage.fire_damage_multiplier();
        mage.consume_spell_buffs();
        assert!(mage.fire_damage_multiplier() < empowered);
    }

    #[test]
    fn overheat_and_the_heat_talents_multiply_together() {
        let mut mage = FireMage::new(1);
        let plain = mage.fire_damage_multiplier();
        assert!((plain - 1.0).abs() < 1e-6);

        mage.gain_heat(100.0);
        let overheated = mage.fire_damage_multiplier();
        assert!((overheated - 1.15).abs() < 1e-6, "{overheated}");

        mage.purchase_talent(TalentId::FurnaceOverdrive)
            .expect("affordable");
        let overdriven = mage.fire_damage_multiplier();
        assert!((overdriven - 1.15 * 1.20).abs() < 1e-5, "{overdriven}");
    }

    #[test]
    fn a_guaranteed_critical_strike_is_spent_by_one_spell() {
        let mut mage = FireMage::new(1);
        mage.guaranteed_crit = true;
        assert!(mage.roll_crit());
        assert!(!mage.guaranteed_crit, "the guarantee survived its spell");
    }

    #[test]
    fn inferno_raises_the_critical_strike_chance_while_it_runs() {
        let mut mage = FireMage::new(1);
        assert!((mage.crit_chance() - BASE_CRIT_CHANCE).abs() < 1e-6);
        mage.inferno_crit.start(INFERNO_CRIT_DURATION);
        assert!((mage.crit_chance() - (BASE_CRIT_CHANCE + INFERNO_CRIT_BONUS)).abs() < 1e-6);

        mage.tick(INFERNO_CRIT_DURATION + 0.1);
        assert!((mage.crit_chance() - BASE_CRIT_CHANCE).abs() < 1e-6);
    }

    #[test]
    fn flame_shield_reduces_damage_and_expires() {
        let mut mage = FireMage::new(1);
        assert_eq!(mage.incoming_damage(100.0), 100.0);

        mage.flame_shield.start(5.0);
        assert!((mage.incoming_damage(100.0) - 70.0).abs() < 1e-4);

        mage.tick(5.1);
        assert_eq!(mage.incoming_damage(100.0), 100.0);
    }

    #[test]
    fn phoenix_protocol_saves_once_and_then_goes_on_cooldown() {
        let mut mage = FireMage::new(1);
        assert!(!mage.try_survive_lethal(), "it fired without the talent");

        mage.purchase_talent(TalentId::PhoenixProtocol)
            .expect("affordable");
        assert!(mage.try_survive_lethal());
        assert!(mage.invulnerable.is_active());
        assert_eq!(mage.incoming_damage(1_000.0), 0.0, "not invulnerable");

        assert!(!mage.try_survive_lethal(), "it saved twice in a row");

        mage.tick(PHOENIX_INVULNERABILITY + 0.1);
        assert_eq!(mage.incoming_damage(100.0), 100.0);
        assert!(mage.phoenix_cooldown.is_active());

        mage.tick(PHOENIX_COOLDOWN);
        assert!(mage.try_survive_lethal(), "it never came back");
    }

    #[test]
    fn a_timer_keeps_the_longer_of_two_durations() {
        let mut timer = Timer::idle();
        timer.start(5.0);
        timer.tick(2.0);
        timer.start(1.0);
        assert!(
            (timer.remaining() - 3.0).abs() < 1e-6,
            "a refresh shortened it"
        );
        timer.start(10.0);
        assert!((timer.remaining() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn resetting_returns_a_fight_to_its_start_but_keeps_the_talents() {
        let mut mage = FireMage::new(1);
        mage.purchase_talent(TalentId::InfernalCapacity)
            .expect("affordable");
        mage.gain_heat(100.0);
        mage.gain_energy(50.0);
        let effects = *mage.effects();
        mage.abilities.spend(FireAbility::Meteor, &effects);
        mage.flame_shield.start(5.0);

        mage.reset();
        assert_eq!(mage.heat.value(), 0.0);
        assert_eq!(mage.energy.value(), 0.0);
        assert!(mage.abilities.is_ready(FireAbility::Meteor));
        assert!(!mage.flame_shield.is_active());
        assert_eq!(mage.ignite_max(), 6, "the talent was refunded");
    }

    #[test]
    fn the_global_cooldown_runs_for_half_a_second_and_is_shortened_by_haste() {
        let mut mage = FireMage::new(1);
        assert!(mage.off_global_cooldown());
        assert_eq!(mage.global_cooldown_fraction(), 1.0);

        mage.start_global_cooldown();
        assert!(!mage.off_global_cooldown());
        assert!(mage.global_cooldown_fraction() < 0.01);

        mage.tick(GLOBAL_COOLDOWN * 0.5);
        let half = mage.global_cooldown_fraction();
        assert!((half - 0.5).abs() < 0.05, "{half}");

        mage.tick(GLOBAL_COOLDOWN);
        assert!(mage.off_global_cooldown());
        assert_eq!(mage.global_cooldown_fraction(), 1.0);

        // Haste shortens it, like every other timer the class has.
        mage.purchase_talent(TalentId::LivingInferno)
            .expect("testing mode pays");
        mage.gain_heat(100.0);
        assert!(mage.haste() > 0.0);
        assert!(mage.global_cooldown_duration() < GLOBAL_COOLDOWN);
    }

    #[test]
    fn energy_trickles_in_while_nothing_is_cast() {
        let mut mage = FireMage::new(1);
        mage.tick(10.0);
        assert!(
            (mage.energy.value() - 5.0).abs() < 0.01,
            "{}",
            mage.energy.value()
        );
    }
}
