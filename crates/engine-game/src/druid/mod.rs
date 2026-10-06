//! The Druid: Mana, Astral Power, and the state its thirteen abilities leave behind.
//!
//! ```text
//!   every Druid cast ──► +Astral Power (the ability's own column)
//!                              │
//!                              ▼
//!                   Astral Power 0 ─────────────── 100
//!                              │
//!            press the ASTRAL POWER button ──► armed
//!                              │
//!            the next *compatible* spell ──► spends 100, and is empowered
//!                              │
//!            ┌─────────────────┴──────────────────┐
//!            ▼                                    ▼
//!   damage: Moonfire +50 % and for ever,   healing: +30 % on every
//!   Sunfire +30 % and a conversion,        healing event the spell
//!   Wrath: the debuff goes wide            produces
//! ```
//!
//! # The one rule the resource is built on
//!
//! **Nothing is consumed automatically.** Reaching a hundred Astral Power does nothing at
//! all: no proc, no free empowerment, no hidden timer. The player presses the button, which
//! *arms* the next compatible cast, and the resource is spent by that cast and by nothing
//! else. Pressing the button and then walking away leaves the Druid armed — which is a
//! decision, not a leak.
//!
//! # What is here and what is not
//!
//! This module is the Druid's *state*: two resources and a channel. It is deliberately the
//! same shape as [`crate::warrior::Warrior`] and [`crate::firemage::FireMage`], and for the
//! same reason — the class that is not being played sits at rest and costs a handful of
//! floats.
//!
//! What is **not** here:
//!
//! * The abilities. They are rows of [`crate::firemage::ABILITIES`], like every other
//!   ability in the game, with their costs and their generation in
//!   [`crate::firemage::DruidRow`].
//! * The damage over time and healing over time effects. They are
//!   [`crate::status::Status`] values, applied and ticked by the one status system.
//! * The healing itself. It goes through `GameWorld::heal` and a `Healing` event, exactly as
//!   the Warrior's does.
//! * Nature's Exposure's amplification and Astral Sunfire's conversion. Both happen in
//!   `Simulation::deal_damage`, which is the one place damage is applied.
//!
//! # Tuning
//!
//! Every magnitude the class is made of is a named constant in [`tuning`]. Nothing in the
//! executor, the interface or the tests writes one down.

use crate::combat::Resource;
use crate::firemage::Timer;

/// The Druid's rows of the ability table.
pub mod abilities;

/// Every magnitude the Druid's kit is made of, in one place.
pub mod tuning {
    /// The Druid's Mana pool.
    ///
    /// A flat pool rather than one derived from Intellect: Intellect is already the whole of
    /// the class's throughput through spell power, and making it the size of the fuel tank
    /// as well would be the same stat paid twice.
    pub const MANA_MAXIMUM: f32 = 10_000.0;

    /// Mana regained per second, in and out of combat.
    pub const MANA_REGEN_PER_SECOND: f32 = 150.0;

    /// The share of maximum Mana a lucky Wrath gives back.
    pub const WRATH_MANA_RESTORE: f32 = 0.01;

    /// The chance a successful Wrath gives that Mana back.
    pub const WRATH_MANA_CHANCE: f32 = 0.25;

    /// The most Astral Power a Druid can hold.
    pub const ASTRAL_MAXIMUM: f32 = 100.0;

    /// What arming and then empowering a spell costs.
    ///
    /// The whole bar, which is what makes the button a decision rather than a rhythm.
    pub const ASTRAL_SPEND: f32 = 100.0;

    /// Extra Moonfire impact damage when Astral Power is spent on it.
    pub const ASTRAL_MOONFIRE_DAMAGE: f32 = 0.50;

    /// Extra Sunfire damage — impact and periodic — when Astral Power is spent on it.
    pub const ASTRAL_SUNFIRE_DAMAGE: f32 = 0.30;

    /// Seconds Astral Sunfire's conversion stays on the enemies Sunfire hit.
    pub const ASTRAL_SUNFIRE_DURATION: f32 = 12.0;

    /// The share of damage dealt to an enemy carrying Astral Sunfire that heals the party.
    pub const ASTRAL_SUNFIRE_CONVERSION: f32 = 0.30;

    /// Seconds an empowered Wrath leaves Astral Wrath running.
    pub const ASTRAL_WRATH_DURATION: f32 = 15.0;

    /// Metres around the primary target that Astral Wrath spreads the debuff to.
    pub const ASTRAL_WRATH_RADIUS: f32 = 10.0;

    /// **What Astral Power is worth to every healing spell in the kit.**
    ///
    /// One number, read by one function, applied to every healing event a spell produces —
    /// the direct half, every tick of the heal over time it leaves, every pulse of a
    /// channel. See [`super::Druid::heal_scale`].
    pub const ASTRAL_HEAL_BONUS: f32 = 0.30;

    /// Nature and Arcane damage one stack of Nature's Exposure adds, as a fraction.
    pub const NATURES_EXPOSURE_PER_STACK: f32 = 0.03;

    /// Seconds a stack of Nature's Exposure lasts, refreshed by every Wrath.
    pub const NATURES_EXPOSURE_DURATION: f32 = 15.0;

    /// Extra Nourish healing per Druid heal over time already on the target.
    ///
    /// **Uncapped, deliberately.** Six of them is sixty per cent; there is no ceiling to
    /// reach and no reason to stop stacking heals before pressing it.
    pub const NOURISH_PER_HOT: f32 = 0.10;

    /// The movement speed Wild Rush adds, as a fraction.
    pub const WILD_RUSH_SPEED: f32 = 0.50;

    /// The damage Wild Rush takes away, as a fraction.
    pub const WILD_RUSH_REDUCTION: f32 = 0.30;

    /// Mana costs, one per ability that has one. Read by the ability table itself.
    pub const MOONFIRE_MANA: f32 = 150.0;
    /// Sunfire's Mana cost.
    pub const SUNFIRE_MANA: f32 = 250.0;
    /// Rejuvenation's Mana cost.
    pub const REJUVENATION_MANA: f32 = 300.0;
    /// Regrowth's Mana cost.
    pub const REGROWTH_MANA: f32 = 600.0;
    /// Lifebloom's Mana cost.
    pub const LIFEBLOOM_MANA: f32 = 350.0;
    /// Wild Growth's Mana cost.
    pub const WILD_GROWTH_MANA: f32 = 900.0;
    /// Swiftmend's Mana cost.
    pub const SWIFTMEND_MANA: f32 = 500.0;
    /// Nourish's Mana cost.
    pub const NOURISH_MANA: f32 = 450.0;
    /// Nature's Cleanse's Mana cost.
    pub const NATURES_CLEANSE_MANA: f32 = 200.0;
    /// Tranquility's Mana cost.
    pub const TRANQUILITY_MANA: f32 = 1_500.0;
}

use tuning::*;

/// Astral Power: a bar that is filled by casting and spent by a button press.
///
/// Deliberately not [`crate::combat::Resource`]: that type regenerates on a clock, and the
/// one fact this resource must never acquire is a rate. Astral Power moves when the Druid
/// casts and when the Druid spends, and at no other time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AstralPower {
    value: f32,
    armed: bool,
}

impl Default for AstralPower {
    fn default() -> Self {
        Self::empty()
    }
}

impl AstralPower {
    /// A Druid who has cast nothing yet.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            value: 0.0,
            armed: false,
        }
    }

    /// Sets the stored Astral Power (clamped): an authoritative correction.
    pub fn set(&mut self, value: f32) {
        self.value = if value.is_finite() {
            value.clamp(0.0, self.max())
        } else {
            0.0
        };
    }

    /// How much is banked.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }

    /// The most that can be banked.
    #[must_use]
    pub fn max(&self) -> f32 {
        ASTRAL_MAXIMUM
    }

    /// How full the bar is, in `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        (self.value / ASTRAL_MAXIMUM).clamp(0.0, 1.0)
    }

    /// Whether the next compatible spell will be empowered.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Whether the button can be pressed right now.
    ///
    /// Enough banked, and not already armed: arming twice would spend nothing and say
    /// nothing, and a button that answers "you already did that" is better than one that
    /// silently does nothing.
    #[must_use]
    pub fn can_arm(&self) -> bool {
        !self.armed && self.value + f32::EPSILON >= ASTRAL_SPEND
    }

    /// Adds `amount`, and reports how much actually went in.
    ///
    /// What is over the maximum is lost rather than banked, exactly as the Warrior's
    /// resource does: a full bar is the design's signal to press the button.
    pub fn gain(&mut self, amount: f32) -> f32 {
        if !amount.is_finite() || amount <= 0.0 {
            return 0.0;
        }
        let before = self.value;
        self.value = (self.value + amount).min(ASTRAL_MAXIMUM);
        self.value - before
    }

    /// Arms the next compatible spell. Returns whether anything changed.
    ///
    /// **Nothing is spent here.** The resource is spent by the spell that consumes the
    /// arming, which is what makes "press the button, then choose what it empowers" true
    /// rather than a description of a rhythm.
    pub fn arm(&mut self) -> bool {
        if !self.can_arm() {
            return false;
        }
        self.armed = true;
        true
    }

    /// Spends the arming and its cost, if there is one to spend.
    ///
    /// Returns whether the caller is empowered. The one place the resource goes down.
    pub fn consume(&mut self) -> bool {
        if !self.armed || self.value + f32::EPSILON < ASTRAL_SPEND {
            // An arming that can no longer be paid for is dropped rather than left dangling:
            // there is nothing that can take the resource away, so this is a guard on a
            // future one.
            self.armed = false;
            return false;
        }
        self.value = (self.value - ASTRAL_SPEND).max(0.0);
        self.armed = false;
        true
    }

    /// Back to nothing banked and nothing armed.
    pub fn clear(&mut self) {
        *self = Self::empty();
    }
}

/// Tranquility in progress: the channel's own clock, beside the cast bar that drives it.
///
/// The *cast* is an ordinary [`crate::sim::ActiveCast`], so moving breaks it and an
/// interrupt stops it with no second set of rules. What this adds is the pulse: how long
/// until the next one, and whether the channel was paid for with Astral Power.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Channel {
    /// How many pulses have been paid out so far.
    ///
    /// A *count* rather than a countdown, and the pulses are fired against the cast's own
    /// elapsed time. A private clock ticking down by a step that is not exactly 1/60 drifts,
    /// and a six-second channel then pays out five pulses or seven depending on which side
    /// of a millionth of a second the last one falls.
    pub pulses: u32,
    /// Whether the channel was empowered when it started.
    ///
    /// Decided once, when the channel begins, so every pulse of one cast is empowered or
    /// none of them is — which is what "do not multiply the same healing event twice" and
    /// "apply it to every pulse" mean together.
    pub empowered: bool,
    /// Whether a channel is running at all.
    pub active: bool,
}

/// Everything the Druid is, at rest or in a fight.
#[derive(Debug, Clone, PartialEq)]
pub struct Druid {
    /// The casting resource.
    pub mana: Resource,
    /// The empowerment resource.
    pub astral: AstralPower,
    /// Tranquility's channel, if one is running.
    pub channel: Channel,
    /// How long the Druid has been out of a global cooldown, unused today.
    ///
    /// Kept so the class has somewhere to grow a clock without a structural change; ticked
    /// to rest and read by nothing.
    idle: Timer,
}

impl Default for Druid {
    fn default() -> Self {
        Self::new()
    }
}

impl Druid {
    /// A Druid at rest: full Mana, no Astral Power, nothing channelling.
    #[must_use]
    pub fn new() -> Self {
        Self {
            mana: Resource::new(MANA_MAXIMUM, MANA_REGEN_PER_SECOND),
            astral: AstralPower::empty(),
            channel: Channel::default(),
            idle: Timer::idle(),
        }
    }

    /// **What Astral Power multiplies a healing event by.**
    ///
    /// The one function every Druid heal goes through — the direct half of Regrowth, every
    /// tick of a Rejuvenation, Lifebloom's bloom, every pulse of a Tranquility. One place,
    /// so a spell cannot be empowered twice and cannot be missed.
    #[must_use]
    pub fn heal_scale(empowered: bool) -> f32 {
        if empowered {
            1.0 + ASTRAL_HEAL_BONUS
        } else {
            1.0
        }
    }

    /// What one stack of Nature's Exposure multiplies incoming Nature and Arcane damage by.
    ///
    /// Read by `Simulation::deal_damage` and by the tooltip, so the number on the screen is
    /// the number the damage used.
    #[must_use]
    pub fn exposure_multiplier(stacks: u8) -> f32 {
        1.0 + f32::from(stacks) * NATURES_EXPOSURE_PER_STACK
    }

    /// Mana one cast of an ability costs.
    #[must_use]
    pub fn mana_cost(ability: crate::firemage::FireAbility) -> f32 {
        ability.definition().druid().mana_cost
    }

    /// Advances everything the Druid owns that is a clock.
    pub fn tick(&mut self, seconds: f32) {
        self.mana.regenerate(seconds);
        self.idle.tick(seconds);
    }

    /// Back to the start of a fight: full Mana, no Astral Power, no channel.
    pub fn reset(&mut self) {
        self.mana.restore();
        self.astral.clear();
        self.channel = Channel::default();
        self.idle.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astral_power_is_never_spent_by_filling_up() {
        let mut astral = AstralPower::empty();
        astral.gain(1_000.0);
        assert_eq!(astral.value(), ASTRAL_MAXIMUM, "the bar overflowed");
        assert!(!astral.is_armed(), "a full bar armed itself");
        assert!(astral.can_arm());
    }

    #[test]
    fn arming_costs_nothing_and_the_spell_pays() {
        let mut astral = AstralPower::empty();
        assert!(!astral.arm(), "an empty bar armed");
        astral.gain(ASTRAL_SPEND);
        assert!(astral.arm());
        assert_eq!(astral.value(), ASTRAL_SPEND, "arming spent the bar");
        assert!(!astral.arm(), "arming twice");

        assert!(astral.consume());
        assert_eq!(astral.value(), 0.0);
        assert!(!astral.is_armed());
        assert!(!astral.consume(), "a second spell was empowered for free");
    }

    #[test]
    fn the_healing_modifier_is_one_number_in_one_place() {
        assert_eq!(Druid::heal_scale(false), 1.0);
        assert!((Druid::heal_scale(true) - 1.30).abs() < 1e-6);
    }

    #[test]
    fn natures_exposure_is_three_per_cent_a_stack_to_forty_five() {
        assert!((Druid::exposure_multiplier(0) - 1.0).abs() < 1e-6);
        assert!((Druid::exposure_multiplier(1) - 1.03).abs() < 1e-6);
        assert!((Druid::exposure_multiplier(15) - 1.45).abs() < 1e-6);
    }

    #[test]
    fn a_reset_refills_the_mana_and_empties_the_bar() {
        let mut druid = Druid::new();
        druid.mana.pay(5_000.0);
        druid.astral.gain(ASTRAL_SPEND);
        druid.astral.arm();
        druid.channel = Channel {
            pulses: 3,
            empowered: true,
            active: true,
        };

        druid.reset();
        assert_eq!(druid.mana.current(), MANA_MAXIMUM);
        assert_eq!(druid.astral.value(), 0.0);
        assert!(!druid.astral.is_armed());
        assert!(!druid.channel.active);
    }
}
