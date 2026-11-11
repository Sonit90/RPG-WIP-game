//! Valor: the Warrior's resource, a bar of five that turns into Battle Fervor when it fills.
//!
//! ```text
//! War Stomp / Colossus Strike / Shield Charge ──► +1 Valor   (one per cast, however many hit)
//!                                                    │
//!                          Valor  ○ ○ ○ ○ ○  ──────► fifth pip
//!                                                    │
//!                                 Valor back to 0, and one stack of Battle Fervor
//!                                 (+10 % damage dealt, −3 % damage taken, 30 s, three stacks)
//! ```
//!
//! # Why it is a count
//!
//! Valor is a number of *casts*, not an amount of anything: the three abilities that pay it
//! each pay exactly one, and the player reads the bar by counting pips. It is an integer so
//! that there is no rounding to disagree about between a server and a client, and so that
//! "five" is a fact the tests can state exactly.
//!
//! # What generates it
//!
//! A **successful cast of War Stomp, Colossus Strike or Shield Charge**, and nothing else.
//! Blocking, parrying and dodging pay nothing. Which abilities pay is a column of the ability
//! table, [`crate::ability::WarriorRow::valor`], so the executor asks the row rather than
//! naming the three.
//!
//! # Why it never decays
//!
//! Deliberately not [`crate::combat::Resource`]: that type regenerates on a clock, and the one
//! fact this resource must never acquire is a rate. Valor moves when the Warrior casts one of
//! its three abilities and when the bar fills, and at no other time.

use super::tuning::VALOR_MAX;

/// The Warrior's resource: `0..=`[`VALOR_MAX`] whole stacks that never decay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Valor {
    stacks: u8,
    /// The stack that fills the bar: [`VALOR_MAX`] unless a talent shortens it. Zero is the
    /// default and means [`VALOR_MAX`], so a derived `Default` stays the untalented bar.
    capacity: u8,
}

impl Valor {
    /// A Warrior who has not cast anything yet.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            stacks: 0,
            capacity: 0,
        }
    }

    /// How many stacks fill the bar.
    #[must_use]
    pub const fn capacity(&self) -> u8 {
        if self.capacity == 0 {
            VALOR_MAX
        } else {
            self.capacity
        }
    }

    /// Changes how many stacks fill the bar (Unbroken Momentum). What is banked is kept,
    /// except that a bar already at the new capacity is left one short of it rather than
    /// being filled by a talent purchase.
    pub fn set_capacity(&mut self, capacity: u8) {
        self.capacity = capacity.clamp(1, VALOR_MAX);
        self.stacks = self.stacks.min(self.capacity.saturating_sub(1));
    }

    /// How many stacks are banked.
    #[must_use]
    pub const fn stacks(&self) -> u8 {
        self.stacks
    }

    /// How full the bar is, in `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        (f32::from(self.stacks) / f32::from(self.capacity())).clamp(0.0, 1.0)
    }

    /// **Adds one stack, and reports whether that filled the bar.**
    ///
    /// When it did, the bar is already back at zero on return: a full Valor is never observed,
    /// because filling it *is* spending it. The caller's only job on `true` is to apply the
    /// Battle Fervor stack the fifth pip bought.
    pub fn gain(&mut self) -> bool {
        let capacity = self.capacity();
        self.stacks = (self.stacks + 1).min(capacity);
        if self.stacks >= capacity {
            self.stacks = 0;
            return true;
        }
        false
    }

    /// Back to empty. For a reset or a hero swap.
    pub fn clear(&mut self) {
        self.stacks = 0;
    }

    /// Sets the stacks directly, clamped to the bar. For arena setup, tests and a client
    /// mirroring what the server says.
    pub fn set(&mut self, stacks: u8) {
        self.stacks = stacks.min(self.capacity());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_empty() {
        assert_eq!(Valor::default().stacks(), 0);
        assert_eq!(Valor::empty().fraction(), 0.0);
    }

    #[test]
    fn the_fifth_stack_empties_the_bar_and_says_so() {
        let mut valor = Valor::empty();
        for expected in 1..VALOR_MAX {
            assert!(!valor.gain(), "stack {expected} does not fill the bar");
            assert_eq!(valor.stacks(), expected);
        }
        assert!(valor.gain(), "the fifth fills it");
        assert_eq!(valor.stacks(), 0, "and it is spent on the same call");
    }

    #[test]
    fn the_fraction_follows_the_stacks() {
        let mut valor = Valor::empty();
        valor.set(2);
        assert!((valor.fraction() - 0.4).abs() < 1e-6);
    }

    #[test]
    fn a_shorter_bar_fills_sooner() {
        let mut valor = Valor::empty();
        valor.set_capacity(4);
        assert_eq!(valor.capacity(), 4);
        assert!(!valor.gain() && !valor.gain() && !valor.gain());
        assert!(valor.gain(), "the fourth fills it");
        assert_eq!(valor.stacks(), 0);
    }

    #[test]
    fn a_set_beyond_the_bar_is_clamped() {
        let mut valor = Valor::empty();
        valor.set(200);
        assert_eq!(valor.stacks(), VALOR_MAX);
        valor.clear();
        assert_eq!(valor.stacks(), 0);
    }
}
