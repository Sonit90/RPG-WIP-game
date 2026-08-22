//! Deflection: the Warrior's resource, and the two passive bonuses that shrink as it fills.
//!
//! ```text
//! Block / Parry / Dodge ──► +0.2 (or +0.4 in Battle Trance)
//!                              │
//!                              ▼
//!                        Deflection 0 ─────────────── 100
//!                              │                       │
//!                    +5% max HP, +10% armour        +0%, +0%
//!                              │                       │
//!                              └──── spend 20 ◄────────┘
//! ```
//!
//! # Why the bonuses run backwards
//!
//! A resource that is only worth spending is a resource a player hoards until the perfect
//! moment. Deflection is worth *not* having: every point of it is a point of maximum health
//! and of armour given up, so sitting at a hundred is sitting at the Warrior's weakest
//! defensive profile with a full bank. Spending gives the defence back on the same tick.
//!
//! The relationship is **continuous and linear**, not a threshold. Spending twenty from
//! ninety-five is worth exactly a fifth of spending twenty from twenty, and a player watching
//! the bar sees the bonus move as it moves. A threshold would make ninety-nine and a hundred
//! feel identical and turn the whole mechanic into a binary.
//!
//! # What generates it
//!
//! A **successful defensive event**, and nothing else — see
//! [`crate::character::DefenseOutcome::is_defensive`]. Not the possession of Block, Parry or
//! Dodge; not an attack that was going to be dodged and killed its attacker first. Because
//! the outcome is produced by exactly one roll per attack, in one place, a duplicated combat
//! event cannot pay twice: there is only ever one outcome to pay for.

/// The most Deflection a Warrior can hold.
pub const DEFLECTION_MAXIMUM: f32 = 100.0;

/// Deflection one successful Block, Parry or Dodge is worth.
pub const DEFLECTION_PER_EVENT: f32 = 0.2;

/// Deflection one successful Block, Parry or Dodge is worth during Battle Trance.
///
/// Double, and deliberately *only* double the resource: Battle Trance does not also heal
/// more per event. It is a resource cooldown and a sustain cooldown at the same time, not
/// two multipliers on the same trigger.
pub const DEFLECTION_PER_EVENT_IN_TRANCE: f32 = 0.4;

/// Extra maximum health at zero Deflection, as a fraction.
pub const PASSIVE_MAX_HEALTH_AT_ZERO: f32 = 0.05;

/// Extra armour at zero Deflection, as a fraction of total armour.
pub const PASSIVE_ARMOR_AT_ZERO: f32 = 0.10;

/// How much Deflection an empowered ability spends.
///
/// Twenty, and it is *not* read from here by gameplay: every empowerable row carries it in
/// [`crate::firemage::WarriorRow::deflection_cost`] so a designer changes one ability without
/// changing them all. This constant is what those rows are written from, and what the tests
/// assert they all still say.
pub const DEFLECTION_SPEND: f32 = 20.0;

/// The Warrior's resource: a value in `0..=100` that never decays.
///
/// Deliberately not [`crate::combat::Resource`]: that type regenerates on a clock, and the
/// one fact this resource must never acquire is a rate. Deflection moves when the Warrior is
/// hit and when the Warrior spends, and at no other time — a decay would quietly hand the
/// passive bonuses back for free and remove the reason to press anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deflection {
    value: f32,
}

impl Default for Deflection {
    fn default() -> Self {
        Self::empty()
    }
}

impl Deflection {
    /// A Warrior who has not been hit yet: no resource, and the full passive bonus.
    #[must_use]
    pub const fn empty() -> Self {
        Self { value: 0.0 }
    }

    /// How much is banked.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }

    /// How full the bar is, in `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        (self.value / DEFLECTION_MAXIMUM).clamp(0.0, 1.0)
    }

    /// Whether the bar is full, which is where the passive bonuses are worth nothing.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.value >= DEFLECTION_MAXIMUM
    }

    /// **What is left of the passive bonuses**, in `0.0..=1.0`.
    ///
    /// One at zero Deflection, zero at a hundred, and linear in between. Both bonuses are
    /// this number times their value at zero, so they cannot drift apart.
    #[must_use]
    pub fn factor(&self) -> f32 {
        1.0 - self.fraction()
    }

    /// Extra maximum health right now, as a fraction.
    #[must_use]
    pub fn max_health_bonus(&self) -> f32 {
        PASSIVE_MAX_HEALTH_AT_ZERO * self.factor()
    }

    /// Extra armour right now, as a fraction.
    #[must_use]
    pub fn armor_bonus(&self) -> f32 {
        PASSIVE_ARMOR_AT_ZERO * self.factor()
    }

    /// Adds `amount`, and reports how much actually went in.
    ///
    /// What is over the maximum is *lost*, not banked: a full bar is the design's signal to
    /// spend, and quietly storing the overflow would remove the signal. Reaching a hundred
    /// carries no penalty of any kind — no damage, no stun, no debuff. The penalty is
    /// already there, and it is the passive bonuses being worth nothing.
    pub fn gain(&mut self, amount: f32) -> f32 {
        if !amount.is_finite() || amount <= 0.0 {
            return 0.0;
        }
        let before = self.value;
        self.value = (self.value + amount).min(DEFLECTION_MAXIMUM);
        self.value - before
    }

    /// Whether `cost` can be paid.
    #[must_use]
    pub fn can_spend(&self, cost: f32) -> bool {
        cost > 0.0 && self.value + f32::EPSILON >= cost
    }

    /// **Spends `cost` if it is there.** Returns whether it was paid.
    ///
    /// All or nothing, and never negative: an ability that cannot afford its empowered form
    /// is cast in its normal form and pays nothing, rather than being refused or leaving a
    /// partial debt.
    pub fn spend(&mut self, cost: f32) -> bool {
        if !self.can_spend(cost) {
            return false;
        }
        self.value = (self.value - cost).max(0.0);
        true
    }

    /// Back to empty. For a reset or a hero swap.
    pub fn clear(&mut self) {
        self.value = 0.0;
    }

    /// Sets the value directly, clamped. For arena setup and tests.
    pub fn set(&mut self, value: f32) {
        self.value = if value.is_finite() {
            value.clamp(0.0, DEFLECTION_MAXIMUM)
        } else {
            0.0
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_defensive_event_is_worth_exactly_two_tenths() {
        let mut deflection = Deflection::empty();
        assert_eq!(deflection.value(), 0.0);
        assert_eq!(deflection.gain(DEFLECTION_PER_EVENT), DEFLECTION_PER_EVENT);
        assert!((deflection.value() - 0.2).abs() < 1e-6);
    }

    #[test]
    fn it_stops_at_a_hundred_and_never_goes_below_zero() {
        let mut deflection = Deflection::empty();
        for _ in 0..1_000 {
            deflection.gain(DEFLECTION_PER_EVENT);
        }
        assert_eq!(deflection.value(), DEFLECTION_MAXIMUM);
        assert!(deflection.is_full());
        assert_eq!(
            deflection.gain(50.0),
            0.0,
            "the overflow is lost, not banked"
        );

        for _ in 0..10 {
            deflection.spend(DEFLECTION_SPEND);
        }
        assert_eq!(deflection.value(), 0.0);
        assert!(!deflection.spend(DEFLECTION_SPEND));
        assert_eq!(deflection.value(), 0.0);
    }

    #[test]
    fn nothing_moves_it_but_a_gain_and_a_spend() {
        // The property that makes the resource a decision: it does not decay, so what is
        // banked is still banked a minute later.
        let mut deflection = Deflection::empty();
        deflection.gain(40.0);
        let before = deflection.value();
        // There is no `tick`. If one is ever added, this test stops compiling.
        assert_eq!(deflection.value(), before);
    }

    #[test]
    fn the_passive_bonuses_scale_continuously_from_full_to_nothing() {
        let expected = [
            (0.0, 0.05, 0.10),
            (25.0, 0.0375, 0.075),
            (50.0, 0.025, 0.05),
            (75.0, 0.0125, 0.025),
            (100.0, 0.0, 0.0),
        ];
        for (value, health, armor) in expected {
            let mut deflection = Deflection::empty();
            deflection.set(value);
            assert!(
                (deflection.max_health_bonus() - health).abs() < 1e-6,
                "{value}: {}",
                deflection.max_health_bonus()
            );
            assert!(
                (deflection.armor_bonus() - armor).abs() < 1e-6,
                "{value}: {}",
                deflection.armor_bonus()
            );
        }
    }

    #[test]
    fn spending_gives_the_bonuses_back_immediately() {
        let mut deflection = Deflection::empty();
        deflection.set(DEFLECTION_MAXIMUM);
        assert_eq!(deflection.max_health_bonus(), 0.0);
        assert!(deflection.spend(DEFLECTION_SPEND));
        // Twenty of a hundred spent is a fifth of both bonuses back, on the same call.
        assert!((deflection.max_health_bonus() - 0.01).abs() < 1e-6);
        assert!((deflection.armor_bonus() - 0.02).abs() < 1e-6);
    }

    #[test]
    fn an_unaffordable_spend_changes_nothing_at_all() {
        let mut deflection = Deflection::empty();
        deflection.set(19.9);
        assert!(!deflection.can_spend(DEFLECTION_SPEND));
        assert!(!deflection.spend(DEFLECTION_SPEND));
        assert!((deflection.value() - 19.9).abs() < 1e-6);
    }

    #[test]
    fn a_nonsense_gain_is_ignored_rather_than_stored() {
        let mut deflection = Deflection::empty();
        for amount in [-1.0, 0.0, f32::NAN, f32::INFINITY] {
            assert_eq!(deflection.gain(amount), 0.0, "{amount}");
        }
        assert_eq!(deflection.value(), 0.0);
    }
}
